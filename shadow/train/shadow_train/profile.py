"""shadow_train.profile — Python loader for the game-profile JSON pair.

Mirrors `src/profile.rs`'s semantics (see `docs/game-profiles.md` for the
design contract this and the Rust loader both implement): per-game knowledge
is DATA, not compiled constants.

  library/<game>/family.json           — port-independent vocabulary:
    roster (id -> name/select_slot/boss), move/attack class lists, block
    style. Shared by every port of the game and stamped into trained models.
  library/<game>/<game>.profile.json   — one PORT: core identity, memory map
    (endianness, fighter blocks + field offsets, named globals), the
    controllable-gate condition list, enforcement values, stage/opponent
    selector, feature calibration, attack-class -> button-chord table.

Game directory resolution (mirrors `shadow/play.py`'s REPO_ROOT convention):
  1. an explicit `game_dir` argument to `load()`/`get()`
  2. the `RUSTRETRO_GAME_DIR` environment variable
  3. `<repo_root>/library/asurabld`, where repo_root is resolved from this
     file's own path (shadow_train/ -> train/ -> shadow/ -> repo root)

This module does no disk I/O at import time (`get()` lazily loads and caches
on first call, keyed by resolved directory) so importing it is cheap and
side-effect-free; only calling `get()`/`load()` touches the filesystem.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional, Union

# shadow_train/profile.py -> shadow_train/ -> train/ -> shadow/ -> repo root
REPO_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_GAME_DIR = REPO_ROOT / "library" / "asurabld"

__all__ = [
    "REPO_ROOT", "DEFAULT_GAME_DIR", "RosterEntry", "GameProfile",
    "load", "get", "ProfileError",
]


class ProfileError(ValueError):
    """A profile JSON pair failed to parse or cross-validate."""


def _parse_addr(v: Union[str, int]) -> int:
    """Hex-string ("0x403798") or plain int -> int. Matches src/profile.rs's
    HexAddr adapter: strings are ALWAYS hex (an optional 0x/0X prefix is
    stripped, not required), numbers are taken as-is."""
    if isinstance(v, bool):  # bool is an int subclass; JSON never emits this
        raise ProfileError(f"address must be a string or int, got bool: {v!r}")
    if isinstance(v, int):
        return v
    s = str(v).strip()
    if s[:2].lower() == "0x":
        s = s[2:]
    return int(s, 16)


@dataclass(frozen=True)
class RosterEntry:
    id: int
    name: str
    select_slot: Optional[int]
    boss: bool


@dataclass
class GameProfile:
    dir: Path

    # raw parsed JSON, for anything not covered by an accessor below
    family_raw: dict
    port_raw: dict

    # family.json
    family: str
    title: str
    roster: list
    move_classes: list
    attack_classes: list
    block_style: dict

    # <game>.profile.json
    port: str
    core: dict
    requires: dict
    memory_endianness: str
    block1_addr: int
    block2_addr: int
    stride_val: int
    fighter_fields: dict          # name -> (off, size)
    globals_map: dict             # name -> addr
    gate: list
    enforcement: dict
    stage_select: Optional[dict]  # {"global": str, "value_to_home_char": {int: int}}
    calibration: dict             # key -> float/int, JSON-native type preserved
    attack_chords: dict           # class name -> [button names]
    positions: dict                # name -> int

    _char_by_id: dict = field(default_factory=dict, repr=False)

    # ── convenience accessors (mirror GameProfile's methods in profile.rs) ──

    def global_addr(self, name: str) -> Optional[int]:
        return self.globals_map.get(name)

    def block1(self) -> int:
        return self.block1_addr

    def block2(self) -> int:
        return self.block2_addr

    def stride(self) -> int:
        return self.stride_val

    def field_off(self, name: str) -> Optional[tuple]:
        """(offset, size) for a fighter field name, or None."""
        return self.fighter_fields.get(name)

    def char_name(self, char_id: int) -> str:
        return self._char_by_id.get(char_id, f"c{char_id}")

    def matchup_slug(self, me: Optional[int], opp: Optional[int]) -> str:
        if me is None and opp is None:
            return "all"
        if opp is None:
            return self.char_name(me)
        if me is None:
            return f"any-vs-{self.char_name(opp)}"
        return f"{self.char_name(me)}-vs-{self.char_name(opp)}"

    def stage_value_for_opponent(self, opp: int) -> Optional[int]:
        """Selector value to freeze to fight `opp` next (None if no value)."""
        if not self.stage_select:
            return None
        for v, home in self.stage_select["value_to_home_char"].items():
            if home == opp:
                return v
        return None

    def opponent_for_stage_value(self, v: int) -> Optional[int]:
        if not self.stage_select:
            return None
        return self.stage_select["value_to_home_char"].get(v)

    def calibration_value(self, key: str):
        return self.calibration.get(key)


def load(game_dir: Optional[Union[str, Path]] = None) -> GameProfile:
    """Parse family.json + <dir-name>.profile.json (falling back to the
    single *.profile.json in the directory) and cross-validate them, exactly
    like `profile::GameProfile::load` in src/profile.rs. Raises ProfileError
    on any mismatch."""
    if game_dir is None:
        game_dir = os.environ.get("RUSTRETRO_GAME_DIR") or DEFAULT_GAME_DIR
    game_dir = Path(game_dir)

    fam_path = game_dir / "family.json"
    try:
        family_raw = json.loads(fam_path.read_text())
    except OSError as e:
        raise ProfileError(f"{fam_path}: {e}") from e

    stem = game_dir.name
    prof_path = game_dir / f"{stem}.profile.json"
    if not prof_path.is_file():
        candidates = sorted(game_dir.glob("*.profile.json"))
        if not candidates:
            raise ProfileError(f"{game_dir}: no *.profile.json found")
        prof_path = candidates[0]
    try:
        port_raw = json.loads(prof_path.read_text())
    except OSError as e:
        raise ProfileError(f"{prof_path}: {e}") from e

    if port_raw["family"] != family_raw["family"]:
        raise ProfileError(
            f"profile family {port_raw['family']!r} != "
            f"family.json {family_raw['family']!r}"
        )

    mem = port_raw["memory"]
    blocks = mem["blocks"]
    block1_addr = _parse_addr(blocks["block1"])
    block2_addr = _parse_addr(blocks["block2"])
    stride_val = _parse_addr(blocks["stride"])

    fighter_fields = {
        f["name"]: (_parse_addr(f["off"]), int(f["size"]))
        for f in mem["fighter_fields"]
    }
    globals_map = {name: _parse_addr(addr) for name, addr in mem["globals"].items()}

    gate = port_raw.get("gate", [])
    for cond in gate:
        g = cond.get("global")
        if g is not None and g not in globals_map:
            raise ProfileError(f"gate condition names unknown global {g!r}")

    attack_classes = list(family_raw["attack_classes"])
    attack_chords = dict(port_raw.get("attack_chords", {}))
    for cls in attack_chords:
        if cls not in attack_classes:
            raise ProfileError(f"attack_chords names unknown class {cls!r}")

    roster = [
        RosterEntry(
            id=r["id"], name=r["name"],
            select_slot=r.get("select_slot"),
            boss=bool(r.get("boss", False)),
        )
        for r in family_raw["roster"]
    ]
    char_by_id = {r.id: r.name for r in roster}

    stage_select = None
    ss = port_raw.get("stage_select")
    if ss:
        stage_select = {
            "global": ss["global"],
            "value_to_home_char": {
                int(k): v for k, v in ss["value_to_home_char"].items()
            },
        }

    return GameProfile(
        dir=game_dir,
        family_raw=family_raw,
        port_raw=port_raw,
        family=family_raw["family"],
        title=family_raw.get("title", ""),
        roster=roster,
        move_classes=list(family_raw["move_classes"]),
        attack_classes=attack_classes,
        block_style=dict(family_raw.get("block", {"style": "back_hold"})),
        port=port_raw["port"],
        core=dict(port_raw.get("core", {})),
        requires=dict(port_raw.get("requires", {})),
        memory_endianness=mem.get("endianness", "big"),
        block1_addr=block1_addr,
        block2_addr=block2_addr,
        stride_val=stride_val,
        fighter_fields=fighter_fields,
        globals_map=globals_map,
        gate=gate,
        enforcement=dict(port_raw.get("enforcement", {})),
        stage_select=stage_select,
        calibration=dict(port_raw.get("calibration", {})),
        attack_chords=attack_chords,
        positions=dict(port_raw.get("positions", {})),
        _char_by_id=char_by_id,
    )


_CACHE: dict = {}


def get(game_dir: Optional[Union[str, Path]] = None) -> GameProfile:
    """Cached `load()` — repeated calls with the same (resolved) directory
    return the same GameProfile instance instead of re-parsing the JSON."""
    resolved = Path(game_dir) if game_dir is not None else Path(
        os.environ.get("RUSTRETRO_GAME_DIR") or DEFAULT_GAME_DIR
    )
    resolved = resolved.resolve()
    prof = _CACHE.get(resolved)
    if prof is None:
        prof = load(resolved)
        _CACHE[resolved] = prof
    return prof
