"""Live memory-RE session protocols, promoted out of heredocs.

Every one of these was re-typed from scratch across at least four RE
sessions (asurabld W1/W2, the sek-bus-bridge pass, mk2 genesis W1/W2 —
see `.claude/skills/re-probe/SKILL.md` for the prose protocol crib and
`library/mk2/mk2-genesis.md` "Session craft" for the gotcha evidence this
module encodes). `Probe` wraps an `McpClient` with the session moves that
kept recurring; the module-level functions are the pure byte-diffing
kernels those moves feed into.

Three lessons worth internalizing before using this, because getting them
wrong wastes a live session, not just a re-read:

  1. PHASE DISCIPLINE. A read is only meaningful once you know whether the
     world is actually running. `Probe.running()` is the free-running-byte
     oracle (a sub-second timer, a frame counter — anything that ticks only
     while live). Don't trust a "controllable" gate alone: in-game pause can
     be invisible to it (see mk2-genesis.md's committed-arena-was-paused
     story) while the oracle byte still tells the truth.
  2. WRITE-TESTS BEAT CORRELATION. A candidate address is only verified once
     writing it produces the predicted observable (teleport, KO, re-render).
     A written value that appears to "revert" on the next read may instead
     be getting CONSUMED (a timer ticking down from what you just wrote) —
     read the post-write trajectory before declaring a disproof.
  3. DERIVED ECHOES. Toggle-intersect (cycle a setting, intersect the
     changed-byte sets across trials) finds the *rendered label* as often as
     the *source flag* — both change on every toggle. Only a write-test that
     provokes re-derivation (or fails to) tells you which one you found.

Dependencies: stdlib only (project convention — see dataset.py, mcpclient.py).
"""

from __future__ import annotations

import time

from .mcpclient import McpClient, McpError

__all__ = [
    "Probe",
    "diff",
    "static_diff",
    "intersect_changes",
    "lua_macro",
    "BUTTON_MASKS",
]

# RETRO joypad button -> input.set() mask bit (src/lua_engine.rs's input.set;
# same table as the re-probe SKILL's "In-engine menu macros" section).
BUTTON_MASKS = {
    "b": 0x1,
    "y": 0x2,
    "select": 0x4,
    "start": 0x8,
    "up": 0x10,
    "down": 0x20,
    "left": 0x40,
    "right": 0x80,
    "a": 0x100,
    "x": 0x200,
    "l": 0x400,
    "r": 0x800,
}

# read_region is capped server-side; chunk snapshots well under that cap.
_SNAPSHOT_CHUNK = 0x2000


def _mask_of(buttons) -> int:
    """Accept a raw int mask, a single RETRO button name, or a list of
    names (OR'd together) -- e.g. "start" -> 0x8, ["down", "b"] -> 0x21."""
    if isinstance(buttons, int):
        return buttons
    names = [buttons] if isinstance(buttons, str) else list(buttons)
    mask = 0
    for name in names:
        try:
            mask |= BUTTON_MASKS[name]
        except KeyError:
            raise ValueError(
                f"unknown button name {name!r} (valid: {sorted(BUTTON_MASKS)})"
            ) from None
    return mask


def _resolve_region(client: McpClient, region: str) -> dict:
    """Resolve a region NAME (exact) or KIND ("RAM"/"ROM"/"VRAM"/"SRAM", the
    `list_regions` classification -- core-specific names vary, e.g. mk2's
    Genesis WRAM vs. asurabld's "68K RAM") to its `list_regions` summary."""
    regions = client.call("list_regions")
    if not isinstance(regions, list):
        raise McpError(f"list_regions: unexpected result {regions!r}")
    for r in regions:
        if str(r.get("name", "")).lower() == region.lower():
            return r
    candidates = [r for r in regions if str(r.get("kind", "")).lower() == region.lower()]
    if candidates:
        # Multiple regions can share a kind (e.g. battery SRAM + main RAM
        # both reporting "RAM"); the biggest one is the main bank.
        return max(candidates, key=lambda r: r.get("size", 0))
    names = [r.get("name") for r in regions]
    raise McpError(f"no region named or kinded {region!r} (have: {names})")


def _state_kwargs(spec: str) -> dict:
    """`spec` is a save-state slot number (as a string/int) or a file path
    -- same convention as `McpClient.load_state`."""
    try:
        return {"slot": int(spec)}
    except (TypeError, ValueError):
        return {"path": spec}


class Probe:
    """Wraps an `McpClient` with the session protocols that got re-typed in
    heredocs across every live-RE pass. One `Probe` = one memory region of
    interest (default the main system RAM, auto-resolved via `list_regions`
    so addresses stay guest-absolute across cores without hand-copying a
    base).
    """

    def __init__(self, url_or_port, region: str = "RAM", region_base: int | None = None):
        if isinstance(url_or_port, int) or (
            isinstance(url_or_port, str) and url_or_port.isdigit()
        ):
            url = f"http://127.0.0.1:{url_or_port}/mcp"
        else:
            url = url_or_port
        self.client = McpClient(url, client_name="re-probe")
        self._writes_armed = False
        resolved = _resolve_region(self.client, region)
        self.region_name = resolved["name"]
        self.region_size = resolved["size"]
        self.region_base = resolved["addr_start"] if region_base is None else region_base

    # ── write gate (lazy: only the first write/load pays the round-trip) ──
    def _ensure_writes(self) -> None:
        if not self._writes_armed:
            self.client.enable_writes()
            self._writes_armed = True

    # ── reads/writes ───────────────────────────────────────────────────────
    def rd8(self, addr: int) -> int:
        return self.client.read_memory(addr, 1)[0]

    def rd16(self, addr: int, little: bool = True) -> int:
        b = self.client.read_memory(addr, 2)
        return (b[0] | (b[1] << 8)) if little else ((b[0] << 8) | b[1])

    def wr8(self, addr: int, v: int) -> None:
        self._ensure_writes()
        self.client.write_memory(addr, 1, v)

    # ── snapshots ──────────────────────────────────────────────────────────
    def snapshot(self, start: int | None = None, length: int | None = None) -> bytes:
        """Chunked `read_region` over this probe's region. Default is the
        whole region. Returned bytes start at region offset `start` (0 by
        default) -- add `self.region_base` to get guest-absolute addresses,
        which is exactly what `diff`/`static_diff`/`intersect_changes`'s
        `base` argument is for."""
        start = 0 if start is None else start
        if length is None:
            length = self.region_size - start
        out = bytearray()
        while len(out) < length:
            n = min(_SNAPSHOT_CHUNK, length - len(out))
            r = self.client.call(
                "read_region", region_name=self.region_name, offset=start + len(out), len=n
            )
            if "hex" not in r:
                raise McpError(f"read_region {self.region_name}: {r}")
            out += bytes.fromhex(r["hex"].replace(" ", ""))
        return bytes(out)

    def stable_snapshot(self, delay: float = 1.0) -> tuple[bytes, set[int]]:
        """Two snapshots `delay` seconds apart -> (first snapshot, set of
        offsets that read identically both times). Feed the pair into
        `static_diff` across two different game states to find config/flag
        bytes while pruning animation/timer noise."""
        a = self.snapshot()
        time.sleep(delay)
        b = self.snapshot()
        n = min(len(a), len(b))
        stable = {i for i in range(n) if a[i] == b[i]}
        return a, stable

    # ── I/O ────────────────────────────────────────────────────────────────
    def screenshot(self, path: str, pause: bool = True) -> None:
        """Screenshots as eyes: pause first for a stable frame (menus/attract
        can otherwise blink mid-capture), shoot, and stay paused so a
        follow-up read sees the same frame the screenshot shows. NOTE: right
        after `load()`, RAM is restored immediately but `app://screen` still
        shows the last-rendered frame until the core runs >=1 frame -- if you
        just loaded a state, `running()`/step once before trusting the shot."""
        if pause:
            self.client.pause()
        self.client.screenshot(path)

    def press(self, buttons, frames: int = 3, port: int = 0) -> dict:
        if isinstance(buttons, str):
            buttons = [buttons]
        return self.client.press(buttons, frames=frames, port=port)

    # ── save states ────────────────────────────────────────────────────────
    def load(self, state_path) -> dict:
        """`load_state` is write-gated (it replaces the whole game state);
        arm writes lazily on first use, same as `wr8`."""
        self._ensure_writes()
        return self.client.call("load_state", **_state_kwargs(state_path))

    def save(self, state_path) -> dict:
        """`save_state` is NOT write-gated."""
        return self.client.call("save_state", **_state_kwargs(state_path))

    # ── phase oracle ───────────────────────────────────────────────────────
    def running(self, oracle_addr: int, settle: float = 0.15) -> bool:
        """Is the world actually advancing? Read a free-running byte (a
        sub-second timer, a frame counter -- anything that ticks only while
        live), wait `settle` seconds, read again. True if it moved. Prefer
        this over any "controllable"/gate flag alone: in-game pause can be
        invisible to a gate that was only ever tested against menu states
        (see mk2-genesis.md's committed-arena-was-paused story)."""
        a = self.rd8(oracle_addr)
        time.sleep(settle)
        b = self.rd8(oracle_addr)
        return a != b


# ── pure byte-diffing kernels (no MCP; unit-testable on synthetic data) ────


def diff(a: bytes, b: bytes, base: int = 0) -> list[tuple[int, int, int]]:
    """Byte-for-byte diff of two same-region snapshots. `base` shifts offsets
    to guest-absolute addresses (e.g. `probe.region_base`)."""
    n = min(len(a), len(b))
    return [(base + i, a[i], b[i]) for i in range(n) if a[i] != b[i]]


def static_diff(
    probe_a_pair: tuple[bytes, set[int]],
    probe_b_pair: tuple[bytes, set[int]],
    base: int = 0,
) -> list[tuple[int, int, int]]:
    """Static-diff protocol: given two `Probe.stable_snapshot()` pairs taken
    in two different game states, return bytes that are BOTH stable within
    each state AND differ between the states. This is the config/flag
    finder -- it prunes animation and timer noise (stable-within-a-state
    excludes them) while still requiring a real difference across states."""
    bytes_a, stable_a = probe_a_pair
    bytes_b, stable_b = probe_b_pair
    common = stable_a & stable_b
    out = []
    n = min(len(bytes_a), len(bytes_b))
    for i in sorted(common):
        if i < n and bytes_a[i] != bytes_b[i]:
            out.append((base + i, bytes_a[i], bytes_b[i]))
    return out


def intersect_changes(snapshots: list[bytes], base: int = 0) -> list[tuple[int, list[int]]]:
    """Toggle-intersect protocol: given snapshots taken across N consecutive
    trials (e.g. cycling a menu setting, or single steps of a walk), return
    (addr, [values across all snapshots]) for every offset that changed at
    EVERY consecutive step. This is a candidate NARROWER, not a verifier --
    a rendered label byte survives this exactly as well as its source flag
    does; only a write-test (does poking it re-derive the label, or not)
    tells the two apart."""
    if len(snapshots) < 2:
        return []
    n = min(len(s) for s in snapshots)
    out = []
    for i in range(n):
        values = [s[i] for s in snapshots]
        if all(values[k] != values[k + 1] for k in range(len(values) - 1)):
            out.append((base + i, values))
    return out


def lua_macro(seq, port: int = 0, timeout_frames: int | None = None) -> str:
    """Build an in-engine frame-scheduled input macro (the re-probe SKILL's
    "In-engine menu macros" -- beats MCP round-trip latency for anything
    sequential, because real time keeps elapsing between Python calls and
    every multi-step menu script needs slack for it otherwise).

    `seq` is `[(at_frame, mask_or_buttons, hold_frames), ...]`; each button
    entry accepts a raw int mask, a single RETRO button name, or a list of
    names (see `BUTTON_MASKS`). Returns the Lua SOURCE STRING -- run it via
    `client.call("run_lua", script=lua_macro(...))`; nothing is executed
    here. The generated script sets a `done` flag once `timeout_frames`
    frames have elapsed (default: last scheduled event's end + 20 frames of
    slack) so it terminates instead of running forever.
    """
    entries = []
    max_end = 0
    for at_frame, buttons, hold in seq:
        mask = _mask_of(buttons)
        entries.append(f"{{at={at_frame}, mask=0x{mask:X}, hold={hold}}}")
        max_end = max(max_end, at_frame + hold)
    if timeout_frames is None:
        timeout_frames = max_end + 20
    seq_src = ",\n    ".join(entries)
    return f"""local seq = {{
    {seq_src}
}}
local port = {port}
local f0 = emu.framecount()
local done = false
event.onframeend(function()
  if done then return end
  local f = emu.framecount() - f0
  for _, s in ipairs(seq) do
    if f >= s.at and f < s.at + s.hold then input.set(port, s.mask) end
  end
  if f > {timeout_frames} then done = true end
end)
"""
