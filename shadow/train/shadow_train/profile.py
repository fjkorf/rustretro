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

    # RECORDER_V3.md §2 additions (all optional; absent = today's meaning).
    record_globals: list = field(default_factory=list)     # [{"name":..,"size":..}, ...]
    recorded_globals: list = field(default_factory=list)   # §1.2 rule 2 union, gate-order
                                                             # first then record_globals order
    hitstun_sources: Optional[dict] = None                  # block name -> global name
    id_map: dict = field(default_factory=dict)              # raw char id (int) -> canonical id

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

    def canon_char_id(self, raw: int) -> int:
        """Raw RAM char id -> canonical family.json roster id (RECORDER_V3.md
        §6). Identity when `id_map` is absent or has no entry for `raw` --
        mirrors `profile::GameProfile::canon_char_id` in src/profile.rs
        exactly (same id_map, same fallback). The ONE Python call site is
        `dataset._decisions_for_round`, building `Decision.me_char/opp_char`."""
        return self.id_map.get(raw, raw)


def _resolve_game_dir(game_dir: Path) -> tuple[Path, Path]:
    """(family_dir, profile_path) for `game_dir`, mirroring
    `profile::GameProfile::resolve_game_dir` in src/profile.rs verbatim
    (RECORDER_V3.md §5.2) -- same three cases, same error text, so a
    `--game` path behaves identically whichever loader reads it."""
    if game_dir.is_dir():
        fam_dir = game_dir
        stem = fam_dir.name
        default_path = fam_dir / f"{stem}.profile.json"
        if default_path.is_file():
            return fam_dir, default_path

        candidates = sorted(fam_dir.glob("*.profile.json"))
        if not candidates:
            raise ProfileError(f"{fam_dir}: no *.profile.json found")
        if len(candidates) == 1:
            return fam_dir, candidates[0]

        # file_stem("mk2.profile.json") is "mk2.profile" -- trim ".profile"
        # so the suggestion names the port segment, not the raw stem.
        stems = [p.name.removesuffix(".profile.json") for p in candidates]
        raise ProfileError(
            f"{fam_dir}: multiple port profiles and no {stem}.profile.json "
            f"default — select one: --game {fam_dir}/{'|'.join(stems)}"
        )

    parent = game_dir.parent
    if parent.is_dir():
        selector = game_dir.name
        direct = parent / f"{selector}.profile.json"
        if direct.is_file():
            return parent, direct

        matches = []
        all_candidates = sorted(parent.glob("*.profile.json"))
        for c in all_candidates:
            try:
                obj = json.loads(c.read_text())
            except (OSError, json.JSONDecodeError):
                continue
            if obj.get("port") == selector:
                matches.append(c)

        if not matches:
            stems = sorted(p.name.removesuffix(".profile.json") for p in all_candidates)
            ports = []
            for c in all_candidates:
                try:
                    ports.append(json.loads(c.read_text()).get("port"))
                except (OSError, json.JSONDecodeError):
                    pass
            available = sorted(stems) + sorted(p for p in ports if p)
            available_str = "/".join(available) if available else "none"
            raise ProfileError(
                f"{parent}: no port {selector!r} (no {selector}.profile.json and "
                f"no profile with \"port\": \"{selector}\"); available: {available_str}"
            )
        if len(matches) == 1:
            return parent, matches[0]
        raise ProfileError(
            f"{parent}: port {selector!r} is ambiguous: "
            + ", ".join(m.name for m in matches)
        )

    raise ProfileError(f"--game {game_dir}: no such game directory")


def load(game_dir: Optional[Union[str, Path]] = None) -> GameProfile:
    """Parse family.json + the selected port profile (RECORDER_V3.md §5.2's
    directory-or-port-segment resolution, see `_resolve_game_dir`) and
    cross-validate them, exactly like `profile::GameProfile::load` in
    src/profile.rs. Raises ProfileError on any mismatch."""
    if game_dir is None:
        game_dir = os.environ.get("RUSTRETRO_GAME_DIR") or DEFAULT_GAME_DIR
    game_dir = Path(game_dir)

    fam_dir, prof_path = _resolve_game_dir(game_dir)

    fam_path = fam_dir / "family.json"
    try:
        family_raw = json.loads(fam_path.read_text())
    except OSError as e:
        raise ProfileError(f"{fam_path}: {e}") from e

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

    # A field is either offset-based ({"off": ...}) or GLOBAL-sourced
    # ({"globals": {"block1": ..., "block2": ...}} — MK2 arcade's world X;
    # RECORDER_V3 §2.5). Offset None marks the global variant; every current
    # Python consumer needs only the NAME (availability) — the recorder
    # resolves addresses on the Rust side and rows carry the values.
    fighter_fields = {
        f["name"]: (_parse_addr(f["off"]) if "off" in f else None, int(f["size"]))
        for f in mem["fighter_fields"]
    }
    globals_map = {name: _parse_addr(addr) for name, addr in mem["globals"].items()}

    gate = port_raw.get("gate", [])
    for cond in gate:
        g = cond.get("global")
        if g is not None and g not in globals_map:
            raise ProfileError(f"gate condition names unknown global {g!r}")

    # §2.1 record_globals: extra per-frame sampled globals beyond gate conds.
    record_globals = list(mem.get("record_globals", []))
    for rg in record_globals:
        if rg["name"] not in globals_map:
            raise ProfileError(f"record_globals names unknown global {rg['name']!r}")

    # §1.2 rule 2's recorded-globals union: gate-condition globals first (in
    # gate order), then record_globals (in its own order), duplicates
    # collapsed to their first position. This is what a v3 row's `globals`
    # object actually contains, and what hitstun_sources is validated against.
    gate_globals = [c["global"] for c in gate if c.get("global")]
    recorded_globals = list(dict.fromkeys(
        gate_globals + [rg["name"] for rg in record_globals]
    ))

    # §2.2 hitstun_sources: block name -> global whose recent change means
    # hitstun. Both names must actually be recorded (else silently reading a
    # global nobody samples).
    hitstun_sources = port_raw.get("hitstun_sources")
    if hitstun_sources:
        for global_name in hitstun_sources.values():
            if global_name not in recorded_globals:
                raise ProfileError(
                    f"hitstun_sources names unrecorded global {global_name!r}"
                )

    # §2.3 id_map: raw RAM char id (decimal-string JSON key) -> canonical
    # family.json roster id. Values must resolve in the family roster.
    id_map_raw = port_raw.get("id_map") or {}
    id_map = {int(k): v for k, v in id_map_raw.items()}
    roster_ids = {r["id"] for r in family_raw["roster"]}
    for canonical_id in id_map.values():
        if canonical_id not in roster_ids:
            raise ProfileError(f"id_map maps to unknown roster id {canonical_id}")

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
        dir=fam_dir,
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
        record_globals=record_globals,
        recorded_globals=recorded_globals,
        hitstun_sources=dict(hitstun_sources) if hitstun_sources else None,
        id_map=id_map,
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
