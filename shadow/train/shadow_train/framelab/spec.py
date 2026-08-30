"""The `framelab` profile block — docs/frames.md's per-port measurement
configuration, as DATA (CLAUDE.md: "never hardcode a game address in code
again"). This module is the ONE place that reads
`profile.port_raw["framelab"]`; `observables.py` and `kit.py` consume the
`FramelabSpec` this produces instead of their own MK2-shaped constants.

What lives here, because docs/frames.md §3.1/§4.1/§4.2 measured all of it
per-port and none of it generalizes across games:

  * **`anchor`** (§4.1) — which field/global is this port's contact signal.
    MK2 arcade: the fighter-struct health `block+0x0E`, explicitly NOT the
    HUD pair (a DRAWN value that smears one hit into ~11 edges).
  * **`observables`** (§4.2) — the ORDERED act-again observable candidates,
    each with its own addressing (a fighter-struct byte range, a named
    fighter field, or the whole struct) and its measured per-probe-shape
    calibration. Order is preference order. A candidate that was measured
    and REJECTED (MK2: `struct_divergence`, `action_counter`) is recorded
    with `status: "disqualified"` and a reason — omission would look like
    an oversight; disqualification is itself a result (docs/frames.md §7:
    "Unmeasurable is a result").
  * **`quiet_frames`** (§4.1) — the multi-hit clustering window.
  * **`rig`** / **`spacing`** — optional rig conventions (per-port walk
    direction preference, the measured spacing-ladder collision floor).
    Nothing in this module requires them; they exist so the schema can
    carry the facts docs/frames.md §5 measured instead of leaving them only
    in comments.

**Absent is a first-class, distinct outcome — never a default.** A port
whose profile carries no `framelab` block (asurabld, mk2 Genesis — neither
has been calibrated) gets `FramelabNotConfigured`, naming the port and what
it lacks, from every entry point in this module. There is no fallback
constant anywhere here: the MK2 numbers this schema was extracted from live
ONLY in `library/mk2/mk2.profile.json` now, and a caller who wants them for
another port must measure that port and add its own block.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Dict, Mapping, Optional, Tuple

__all__ = [
    "FramelabError",
    "FramelabNotConfigured",
    "Addressing",
    "AnchorSpec",
    "ObservableSpec",
    "RigSpec",
    "SpacingSpec",
    "FramelabSpec",
]

_VALID_ANCHOR_SOURCES = ("field", "hitstun_sources")
_VALID_ADDRESSING_KINDS = ("fighter_field", "byte_range", "whole_struct")
_VALID_OBSERVABLE_STATUS = ("active", "disqualified")


class FramelabError(ValueError):
    """The profile's `framelab` block exists but is malformed (a schema bug,
    not an uncalibrated port)."""


class FramelabNotConfigured(FramelabError):
    """This port has no usable framelab data for what was asked — no block
    at all, or the block exists but lacks the specific anchor / observable /
    calibration entry needed. Distinct from `FramelabError` so a caller can
    tell "not calibrated yet" (expected, for most ports) from "calibrated
    but the JSON is broken" (a bug)."""


def _hexint(v: Any) -> int:
    if isinstance(v, bool):
        raise FramelabError(f"expected an address/offset, got bool: {v!r}")
    if isinstance(v, int):
        return v
    s = str(v).strip()
    if s[:2].lower() == "0x":
        s = s[2:]
    return int(s, 16)


@dataclass(frozen=True)
class Addressing:
    """Where one observable's bytes live, relative to a fighter block.

    - `"fighter_field"`: read the profile's own named `memory.fighter_fields`
      entry (`field`) — this is how `pointer_x` addresses the existing
      `via: "object_ptr"` field `x` rather than duplicating its offsets.
    - `"byte_range"`: raw bytes `[off, end)` relative to the fighter block
      base, compared for equality rather than decoded as an integer — MK2's
      walk-velocity word (`0x0B..0x0E`) is a 3-byte run with no numeric
      meaning worth extracting; only "did it change" matters (§4.2).
    - `"whole_struct"`: the entire fighter struct (`block .. block+stride`).
      Offered for schema completeness (a future port's struct-divergence
      observable would use this); MK2 disqualifies it (§4.2).
    """

    kind: str
    field: Optional[str] = None
    off: Optional[int] = None
    end: Optional[int] = None

    @classmethod
    def from_json(cls, d: Mapping[str, Any], *, where: str) -> "Addressing":
        kind = d.get("kind")
        if kind not in _VALID_ADDRESSING_KINDS:
            raise FramelabError(
                f"{where}: addressing 'kind' must be one of "
                f"{_VALID_ADDRESSING_KINDS}, got {kind!r}"
            )
        if kind == "fighter_field":
            field = d.get("field")
            if not field:
                raise FramelabError(
                    f"{where}: addressing kind 'fighter_field' needs a 'field' name"
                )
            return cls(kind=kind, field=field)
        if kind == "byte_range":
            if "off" not in d or "end" not in d:
                raise FramelabError(
                    f"{where}: addressing kind 'byte_range' needs 'off' and 'end'"
                )
            off, end = _hexint(d["off"]), _hexint(d["end"])
            if end <= off:
                raise FramelabError(
                    f"{where}: byte_range end ({end:#x}) must be > off ({off:#x})"
                )
            return cls(kind=kind, off=off, end=end)
        return cls(kind=kind)  # "whole_struct" needs nothing further


@dataclass(frozen=True)
class AnchorSpec:
    """§4.1's contact anchor: exactly one of `field` (a per-fighter struct
    field, PREFERRED — it steps by the true value in one frame) or
    `hitstun_sources` (this fighter's per-block global from the profile's
    existing `hitstun_sources` map — the HUD pair on MK2 arcade, which
    ANIMATES toward the true value and is explicitly NOT preferred, §4.1)."""

    source: str
    field: Optional[str] = None

    @classmethod
    def from_json(cls, d: Mapping[str, Any]) -> "AnchorSpec":
        field = d.get("field")
        uses_hitstun = "hitstun_sources" in d
        if field and uses_hitstun:
            raise FramelabError(
                "framelab.anchor: pick 'field' or 'hitstun_sources', not both"
            )
        if field:
            return cls(source="field", field=field)
        if uses_hitstun:
            return cls(source="hitstun_sources")
        raise FramelabError(
            "framelab.anchor needs 'field' (a fighter-struct field name) or "
            "'hitstun_sources' (use the profile's per-block hitstun_sources)"
        )


@dataclass(frozen=True)
class ObservableSpec:
    """One entry in §4.2's ordered candidate list. `calibration` maps probe
    SHAPE (`"attacker/hit"`, `"defender/hit"`, `"attacker/block"`,
    `"defender/block"` — docs/frames.md §3.1's guarded-defender-differs
    correction is exactly why these four are separate, not one number) to
    its measured `input_latency_frames`."""

    name: str
    status: str
    addressing: Optional[Addressing] = None
    calibration: Dict[str, int] = None  # type: ignore[assignment]
    reason: Optional[str] = None

    def __post_init__(self) -> None:
        if self.calibration is None:
            object.__setattr__(self, "calibration", {})

    def latency_for(self, shape: str) -> int:
        """The measured `input_latency_frames` for this observable under
        `shape`. Declines — never guesses from a different shape or a
        different observable — exactly the failure §3.1 documents ("sizing
        the last from the first produced a confident silent wrong answer")."""
        if self.status != "active":
            raise FramelabNotConfigured(
                f"observable {self.name!r} is disqualified"
                + (f" ({self.reason})" if self.reason else "")
                + " -- it has no calibration to read."
            )
        if shape not in self.calibration:
            raise FramelabNotConfigured(
                f"observable {self.name!r} has no calibration for probe shape "
                f"{shape!r} in this profile's framelab block "
                f"(measured shapes: {sorted(self.calibration) or 'none'})."
            )
        return self.calibration[shape]


@dataclass(frozen=True)
class RigSpec:
    """Optional rig conventions (docs/frames.md §4.2's blocked-direction
    hazard): which walk direction each port should try FIRST (away from the
    opponent), and second."""

    attacker_port: int
    defender_port: int
    walk_directions_by_port: Dict[int, Tuple[str, ...]]


@dataclass(frozen=True)
class SpacingSpec:
    """Optional spacing-ladder evidence (docs/frames.md §5): the measured
    collision floor below which no amount of extra walking closes the gap
    further. Documentary only today — no code path computes with it — kept
    here so the number lives in data next to the rest of this port's
    measurements rather than only in a doc's prose."""

    collision_floor_px: Optional[int] = None
    collision_floor_evidence: str = ""


@dataclass(frozen=True)
class FramelabSpec:
    anchor: AnchorSpec
    quiet_frames: int
    observables: Tuple[ObservableSpec, ...]  # declared order == preference order
    rig: Optional[RigSpec] = None
    spacing: Optional[SpacingSpec] = None

    @classmethod
    def from_profile(cls, profile: Any) -> "FramelabSpec":
        """Parse `profile.port_raw["framelab"]`. Raises `FramelabNotConfigured`
        (never returns a default) when the block, or a piece it needs, is
        absent."""
        port_id = f"{getattr(profile, 'family', '?')}/{getattr(profile, 'port', '?')}"
        raw = (getattr(profile, "port_raw", None) or {}).get("framelab")
        if raw is None:
            raise FramelabNotConfigured(
                f"{port_id}: this port's profile has no `framelab` block, so "
                "the frame lab has no calibrated anchor/observable/probe-shape "
                "data for it (docs/frames.md §3.1 requires per-port "
                "measurement; there is no cross-game default -- CLAUDE.md: "
                "'never hardcode a game address in code again'). Measure this "
                "port and add its `framelab` block, or point the lab at a "
                "port that already has one (currently: library/mk2 arcade)."
            )

        anchor_raw = raw.get("anchor")
        if not anchor_raw:
            raise FramelabNotConfigured(f"{port_id}: framelab block has no 'anchor'")
        anchor = AnchorSpec.from_json(anchor_raw)

        if "quiet_frames" not in raw:
            raise FramelabNotConfigured(
                f"{port_id}: framelab block has no 'quiet_frames' (§4.1's "
                "multi-hit clustering window)"
            )
        quiet_frames = int(raw["quiet_frames"])

        obs_raw = raw.get("observables")
        if not obs_raw:
            raise FramelabNotConfigured(
                f"{port_id}: framelab block has no 'observables'"
            )
        observables = []
        seen: set = set()
        for i, o in enumerate(obs_raw):
            name = o.get("name")
            if not name:
                raise FramelabError(
                    f"{port_id}: framelab.observables[{i}] has no 'name'"
                )
            if name in seen:
                raise FramelabError(
                    f"{port_id}: framelab.observables names {name!r} twice"
                )
            seen.add(name)
            status = o.get("status", "active")
            if status not in _VALID_OBSERVABLE_STATUS:
                raise FramelabError(
                    f"{port_id}: observable {name!r} has unknown status "
                    f"{status!r} (valid: {_VALID_OBSERVABLE_STATUS})"
                )
            reason = o.get("reason")
            addressing = None
            calibration: Dict[str, int] = {}
            if status == "disqualified":
                if not reason:
                    raise FramelabError(
                        f"{port_id}: disqualified observable {name!r} needs a "
                        "'reason' -- a disqualification with no reason is "
                        "indistinguishable from one nobody explained."
                    )
            else:
                addr_raw = o.get("addressing")
                if not addr_raw:
                    raise FramelabNotConfigured(
                        f"{port_id}: observable {name!r} is active but has no "
                        "'addressing'"
                    )
                addressing = Addressing.from_json(
                    addr_raw, where=f"{port_id} observable {name!r}"
                )
                calibration = {
                    str(k): int(v) for k, v in (o.get("calibration") or {}).items()
                }
                if not calibration:
                    raise FramelabNotConfigured(
                        f"{port_id}: observable {name!r} is active but carries "
                        "no 'calibration' -- an uncalibrated observable is not "
                        "usable (docs/frames.md §3.1: 'an uncalibrated run is "
                        "not a run')."
                    )
            observables.append(
                ObservableSpec(
                    name=name, status=status, addressing=addressing,
                    calibration=calibration, reason=reason,
                )
            )

        rig = None
        rig_raw = raw.get("rig")
        if rig_raw:
            wdbp_raw = rig_raw.get("walk_directions_by_port") or {}
            rig = RigSpec(
                attacker_port=int(rig_raw.get("attacker_port", 0)),
                defender_port=int(rig_raw.get("defender_port", 1)),
                walk_directions_by_port={
                    int(k): tuple(v) for k, v in wdbp_raw.items()
                },
            )

        spacing = None
        sp_raw = raw.get("spacing")
        if sp_raw:
            spacing = SpacingSpec(
                collision_floor_px=sp_raw.get("collision_floor_px"),
                collision_floor_evidence=sp_raw.get("collision_floor_evidence", ""),
            )

        return cls(
            anchor=anchor, quiet_frames=quiet_frames,
            observables=tuple(observables), rig=rig, spacing=spacing,
        )

    # ── convenience accessors ────────────────────────────────────────────

    def active_observables(self) -> Tuple[ObservableSpec, ...]:
        """Declared order IS preference order (docs/frames.md §4.2)."""
        return tuple(o for o in self.observables if o.status == "active")

    def observable(self, name: str) -> ObservableSpec:
        for o in self.observables:
            if o.name == name:
                return o
        raise FramelabNotConfigured(
            f"framelab block does not mention observable {name!r} at all "
            f"(known: {[o.name for o in self.observables]})"
        )

    def default_observable_names(self) -> Tuple[str, ...]:
        """The ordered names a caller should measure with when it has no
        stronger opinion — every ACTIVE observable, in preference order."""
        actives = self.active_observables()
        if not actives:
            raise FramelabNotConfigured(
                "framelab block declares no ACTIVE observables -- every "
                "candidate is disqualified, so this port has nothing to "
                "measure act-again with yet."
            )
        return tuple(o.name for o in actives)
