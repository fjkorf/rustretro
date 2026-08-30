"""docs/frames.md §4.2 — the observables, built from a `GameProfile` rather
than from constants in code (CLAUDE.md: "never hardcode a game address in
code again").

§4.2's preference order, per port:

  1. **Fighter-struct divergence** — any byte of the fighter's own struct
     differing between the probe run and the control run.
  2. **`action_counter` (`+0xC0`) edge** — it fires on ENTERING an action,
     which is precisely "this fighter just started doing something".
     Retracted as a *contact* signal, correct as an *act-again* signal.
  3. **Mapped `x`** — on MK2 arcade the POINTER-RESOLVED `obj+0x12` (§5).
     The raw globals `p1_x`/`p2_x` are FORBIDDEN: they name a slot in a
     `0x42`-stride object pool that is not stable across boots, and they read
     a frozen value through holds that visibly moved the fighter
     (`library/mk2/mk2.md`, "Toolkit friction" and criterion 5).

## What that order turns out to be, on MK2 arcade — measured, and inverted

All three were run against `shadow/arenas/mk2/r-v-r.state` (2-human Reptile
mirror). §4.2's order does not survive contact with this port:

  * **`action_counter` (#2) is DISQUALIFIED.** §3.1's zero-point calibration
    was run on it directly, both ports, `max_search=10`: it produced NO
    divergence at all for a held walk in either direction. It fires on
    entering an ATTACK (160 -> 192 on a Reptile HP, live) but not on entering
    a walk, so it cannot answer "can this fighter walk yet". §3.1's rule
    applies verbatim: the probe is not sound on this observable, so nothing
    downstream of it can be trusted.
  * **`struct_divergence` (#1) is DISQUALIFIED for the act-again probe, and
    §4.2's "exceptionally clean ... idle churn = 0 bytes" is an ABSOLUTE-test
    observation that does not transfer to the DIFFERENTIAL one.** Probing the
    defender at N = 3, 11, 19, 26, 40 frames after contact — i.e. deep inside
    blockstun, where the correct answer is FALSE at 3 — the struct diverged
    from the control within 1-2 frames at EVERY one of them, in bytes
    `+0x1C`, `+0x6C`, `+0x70..0x72`, `+0xC0`, `+0xC4..0xC6`. Those fields echo
    the RAW HELD DIRECTION while the fighter is still stunned. A struct-wide
    diff therefore answers "did the input reach the game", not "can this
    fighter act", and in a live sweep it produced a non-monotone predicate
    (`...T.......T.......TT.....T....T`) that no search method can read.
  * **`STRUCT_VELOCITY` (`block + 0x0B..0x0D`) is the sound struct-side
    observable** and is offered here in place of the whole-struct diff. It is
    the fighter's own walk velocity: `00 00 00` standing, `00 fe ff` walking.
    It moves only when the fighter really walks, and its knee/plateau curve
    over 46 candidate N agrees with `pointer_x` on every configuration
    measured.
  * **`pointer_x` (#3) is sound**, exactly as §5 describes.

Independence, for §8.4's cross-method requirement: `action_counter` lives at
`+0xC0`, INSIDE the `0x17A` struct, so `struct_divergence` strictly CONTAINS
it — those two agreeing would be a weaker check than it looks.
`STRUCT_VELOCITY` (fighter struct) and `pointer_x` (object pool) are in
different data structures and are the genuinely independent pair on this port.

Everything in this module reads one composite sample per frame (`sampler`)
and derives every observable from it, so one set of runs serves all of them.

## Where the addresses come from

The object pointer and its `x`/`y`/`cid` offsets are read from the profile
(`memory.blocks.object_ptr` plus the `via: "object_ptr"` fighter fields) by
`ObjectPointer.from_profile`; the constants on `ObjectPointer` are only a
fallback for a profile that predates that schema.

`STRUCT_VELOCITY_RANGE` is the one address in this module that is NOT in any
profile: it was found by this task and has no schema slot yet. It is flagged
at its definition and should move into `mk2.profile.json` as a fighter field
(alongside its evidence in `library/mk2/mk2.md`) rather than stay here.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Callable, Dict, Hashable, Mapping, Optional, Tuple

__all__ = [
    "OBJECT_PTR",
    "STRUCT_DIVERGENCE",
    "STRUCT_VELOCITY",
    "STRUCT_VELOCITY_RANGE",
    "ACTION_COUNTER",
    "POINTER_X",
    "CONTACT_STRUCT_HEALTH",
    "CONTACT_HUD_HEALTH",
    "FighterAddrs",
    "resolve_fighter",
    "make_sampler",
    "make_contact_read",
    "make_arena_verifier",
]

STRUCT_DIVERGENCE = "struct_divergence"
STRUCT_VELOCITY = "struct_velocity"
ACTION_COUNTER = "action_counter"
POINTER_X = "pointer_x"

# MK2 arcade, live-measured (see this module's docstring): the fighter's own
# walk velocity, a 3-byte little-endian-ish run at `block + 0x0B`. Reads
# 00 00 00 standing, 00 fe ff walking left, and a mirrored pattern walking
# right. Like OBJECT_PTR this belongs in the profile once the schema grows a
# place for it; it is NOT in `mk2.profile.json` today.
STRUCT_VELOCITY_RANGE = (0x0B, 0x0E)


def _as_int(v: Any, default: int) -> int:
    if v is None:
        return default
    if isinstance(v, str):
        s = v.strip()
        neg = s.startswith("-")
        if neg:
            s = s[1:]
        n = int(s, 16) if s.lower().startswith("0x") else int(s)
        return -n if neg else n
    return int(v)


@dataclass(frozen=True)
class ObjectPointer:
    """mk2.md: `obj = (u32_le(block - 0x0C) - 0x01000000) >> 3` (a TMS34010
    bit address); valid only inside `0x01000000..0x01400000` — outside that
    range there is no fighter object and no x/y should be emitted.

    The defaults below are the live-verified MK2 arcade values, but they are
    a FALLBACK: `from_profile` reads `memory.blocks.object_ptr` and the
    `via: "object_ptr"` fighter fields when the profile carries them, so a
    port that moves the pointer does not need a code change (CLAUDE.md:
    "address changes go to the profile ... never into code").
    """

    off: int = -0x0C
    size: int = 4
    bias: int = 0x01000000
    shift: int = 3
    valid_lo: int = 0x01000000
    valid_hi: int = 0x01400000
    x_off: int = 0x12
    y_off: int = 0x16
    char_off: int = 0x3E
    span: int = 0x42

    @classmethod
    def from_profile(cls, profile: Any) -> "ObjectPointer":
        blocks = (profile.port_raw.get("memory", {}) or {}).get("blocks", {}) or {}
        decl = blocks.get("object_ptr")
        if not decl:
            return cls()
        lo, hi = decl.get("valid_range", [cls.valid_lo, cls.valid_hi])
        via = {
            f["name"]: f
            for f in (profile.port_raw.get("memory", {}) or {}).get(
                "fighter_fields", []
            )
            if f.get("via") == "object_ptr"
        }
        d = cls()
        return cls(
            off=_as_int(decl.get("off"), d.off),
            size=_as_int(decl.get("size"), d.size),
            valid_lo=_as_int(lo, d.valid_lo),
            valid_hi=_as_int(hi, d.valid_hi),
            char_off=_as_int(decl.get("cid_check_off"), d.char_off),
            x_off=_as_int(via.get("x", {}).get("off"), d.x_off),
            y_off=_as_int(via.get("y", {}).get("off"), d.y_off),
        )


OBJECT_PTR = ObjectPointer()


@dataclass(frozen=True)
class FighterAddrs:
    name: str            # "block1" / "block2"
    base: int
    stride: int
    port: int
    action_counter_off: Optional[int]
    health_off: Optional[int]
    hitstun_source: Optional[int]   # this fighter's per-victim contact global
    ptr: "ObjectPointer" = OBJECT_PTR


def resolve_fighter(profile: Any, block: str, port: int) -> FighterAddrs:
    """Everything the observables need about one fighter, straight from the
    profile: block base, stride, the `action_counter` and `health` field
    offsets, the `hitstun_sources` global for this fighter (§4.1), and the
    object-pointer declaration (§5)."""
    base = profile.block1() if block == "block1" else profile.block2()
    ac = profile.field_off("action_counter")
    hp = profile.field_off("health")
    hs = None
    if profile.hitstun_sources:
        gname = profile.hitstun_sources.get(block)
        if gname:
            hs = profile.global_addr(gname)
    return FighterAddrs(
        name=block,
        base=base,
        stride=profile.stride(),
        port=port,
        action_counter_off=ac[0] if ac else None,
        health_off=hp[0] if hp else None,
        hitstun_source=hs,
        ptr=ObjectPointer.from_profile(profile),
    )


def _u32le(b: bytes) -> int:
    return int.from_bytes(b, "little")


def _resolve_obj(struct_lead: bytes, ptr: ObjectPointer) -> Optional[int]:
    """`struct_lead` is the bytes read from `block + ptr.off`; the pointer is
    its FIRST `ptr.size` bytes (the read may be wider — `make_sampler` reads
    the pointer and the whole struct in one call)."""
    raw = _u32le(struct_lead[: ptr.size])
    if not (ptr.valid_lo <= raw < ptr.valid_hi):
        return None
    return (raw - ptr.bias) >> ptr.shift


def make_sampler(
    fighter: FighterAddrs,
    *,
    ptr: Optional[ObjectPointer] = None,
    include: Tuple[str, ...] = (
        STRUCT_DIVERGENCE,
        STRUCT_VELOCITY,
        ACTION_COUNTER,
        POINTER_X,
    ),
    velocity_range: Tuple[int, int] = STRUCT_VELOCITY_RANGE,
) -> Callable[[Any], Mapping[str, Hashable]]:
    """One composite read per frame -> `{observable_name: hashable value}`.

    Two `read_memory` calls per frame (each ~0.4 ms live, versus ~18 ms for a
    confirmed step), so sampling every frame is free relative to stepping:

      1. `[block-0x0C, block+stride)` — the object pointer AND the whole
         fighter struct in one read.
      2. `[obj, obj+0x42)` — the fighter's object-pool entry, for x/y and the
         `obj+0x3E` staleness cross-check.

    `pointer_x` is `None` when the pointer is out of range OR when
    `obj+0x3E != block+0x0` — §4.2: "a mismatch means the pointer went stale
    and the row must be discarded, not recorded". A `None` here propagates
    into the differential as a value like any other; the caller sees it in
    the trace and can refuse the row.
    """
    ptr = ptr or fighter.ptr
    lead = -ptr.off  # bytes read before `base`

    def sample(session: Any) -> Mapping[str, Hashable]:
        buf = session.read_memory(fighter.base + ptr.off, lead + fighter.stride)
        struct = buf[lead:]
        out: Dict[str, Hashable] = {}
        if STRUCT_DIVERGENCE in include:
            out[STRUCT_DIVERGENCE] = bytes(struct)
        if STRUCT_VELOCITY in include:
            out[STRUCT_VELOCITY] = bytes(struct[velocity_range[0] : velocity_range[1]])
        if ACTION_COUNTER in include:
            out[ACTION_COUNTER] = (
                struct[fighter.action_counter_off]
                if fighter.action_counter_off is not None
                else None
            )
        if POINTER_X in include:
            obj = _resolve_obj(buf[:lead], ptr)
            if obj is None:
                out[POINTER_X] = None
            else:
                ent = session.read_memory(obj, ptr.span)
                cid = ent[ptr.char_off]
                if cid != struct[0]:
                    out[POINTER_X] = None  # stale pointer -> discard, never record
                else:
                    out[POINTER_X] = int.from_bytes(
                        ent[ptr.x_off : ptr.x_off + 2], "little"
                    )
        return out

    return sample


CONTACT_STRUCT_HEALTH = "struct_health"
CONTACT_HUD_HEALTH = "hud_health"


def make_contact_read(
    fighter: FighterAddrs, *, source: str = CONTACT_STRUCT_HEALTH
) -> Callable[[Any], Hashable]:
    """§4.1's contact anchor, read for THIS fighter (the victim).

    Two per-victim damage readings exist on MK2 arcade, and they are NOT
    interchangeable for anchoring — this was measured live, and it is a
    correction to §4.1:

      * `struct_health` (`block + 0x0E`) — the damage register. Steps by the
        whole damage amount in ONE frame: an HP that deals 11 goes
        161 -> 150 on a single frame, and a blocked one 161 -> 158.
        **One edge per contact**, which is what §4.1's "last contact before
        the quiet window" rule assumes.
      * `hud_health` (`hitstun_sources`, the `0xBCA0`/`0xBC88` pair that
        §4.1 names) — the drawn BAR, which ANIMATES toward the register at
        1 unit per frame. The same single HP produced ELEVEN consecutive
        changes (161,160,...,150) starting on the same frame as the register
        edge. Its FIRST edge is a correct contact frame; its clustering is
        not — §4.1's rule would anchor 10 frames late on a single hit and
        would count a one-hit move as 11 hits.

    So the default here is `struct_health`, and `hud_health` is kept for the
    cross-check that established the difference. `hit_counter 0xD3FE` is
    offered by neither: live 2-human testing found it does not move for hits
    landed on P2 (mk2.md, "Contact-signal correction"), and it is not in the
    shipped profile.
    """
    if source == CONTACT_STRUCT_HEALTH:
        if fighter.health_off is None:
            raise ValueError(f"{fighter.name} has no `health` fighter field")
        addr = fighter.base + fighter.health_off
    elif source == CONTACT_HUD_HEALTH:
        if fighter.hitstun_source is None:
            raise ValueError(
                f"{fighter.name} has no hitstun_sources global in the profile, "
                "so this port has no contact signal to anchor on. "
                "docs/frames.md §4.1: that is a legitimate table entry "
                "(advantage unmeasurable), not a reason to substitute a proxy."
            )
        addr = fighter.hitstun_source
    else:
        raise ValueError(f"unknown contact source {source!r}")

    def read(session: Any) -> Hashable:
        return session.read_memory(addr, 1)[0]

    return read


def make_arena_verifier(
    profile: Any,
    *,
    expect: Mapping[str, int],
    ptr: Optional[ObjectPointer] = None,
) -> Callable[[Any], bool]:
    """§3.4: "Arena liveness re-verified after EVERY `load_state`."

    `expect` names the arena's identity (from its `.meta.json` sidecar, or
    read once at capture): `char_id_block1`, `char_id_block2`,
    `health_block1`, `health_block2`. The verifier checks all of them plus
    the profile's own round-live globals and BOTH object pointers resolving
    in range with matching char ids — the last of which is exactly the
    liveness check `p1_x` was wrongly used for before (mk2.md, "Pointer
    hygiene").

    It deliberately does NOT re-run a walk test: that costs frames and would
    perturb the state being measured. The walk-based `inputs_live` assertion
    belongs at capture and at session start; what this catches after every
    load is the cheap-but-real failure — the load did not land, or landed on
    a state that is no longer a live round.
    """
    ptr = ptr or ObjectPointer.from_profile(profile)
    b1, b2 = profile.block1(), profile.block2()
    health_off = profile.field_off("health")[0]
    round_over = profile.global_addr("round_over")

    def verify(session: Any) -> bool:
        for block, base in (("block1", b1), ("block2", b2)):
            buf = session.read_memory(base + ptr.off, -ptr.off + profile.stride())
            struct = buf[-ptr.off :]
            want_char = expect.get(f"char_id_{block}")
            if want_char is not None and struct[0] != want_char:
                return False
            want_hp = expect.get(f"health_{block}")
            if want_hp is not None and struct[health_off] != want_hp:
                return False
            obj = _resolve_obj(buf[: -ptr.off], ptr)
            if obj is None:
                return False
            if session.read_memory(obj + ptr.char_off, 1)[0] != struct[0]:
                return False
        if round_over is not None and session.read_memory(round_over, 1)[0] != 0:
            return False
        return True

    return verify
