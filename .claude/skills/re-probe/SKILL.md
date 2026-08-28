---
name: re-probe
description: Launch a headless RustRetro instance and run a live memory-RE session with the proven protocols (phase discipline, snapshot-diff, write-tests, menu macros)
---

# re-probe — live memory-RE session protocol

## Launch (never port 4025 — that's the user's session; agents use 4030+)

```sh
./target/release-dev/rustretro --core <CORE> --rom <ROM> --game library/<family>[/<port>] \
  --headless --mcp-port 4030 [--pace 0]
```

- Build first: `cargo build --profile release-dev` (`cargo test` does NOT rebuild the binary).
- `--pace 1` (default) = real-time 60fps — use for anything with timed input.
  `--pace 0` = uncapped — use for "let the game reach phase X fast", never for interactive probing.
- MCP client: `shadow/train/.venv/bin/python3`, then
  `from shadow_train.mcpclient import McpClient; c = McpClient("http://127.0.0.1:4030/mcp")`,
  `c.call(tool, **kwargs)`. `run_lua` takes `script=...` → `{'ok','output'}`.
  Writes/load_state need `c.call("enable_writes")` first.
- `shadow_train.re.Probe` is this protocol crib's EXECUTABLE counterpart —
  `from shadow_train.re import Probe; p = Probe(4030)` wraps an `McpClient`
  with the moves below (`rd8`/`rd16`/`wr8`, `snapshot`/`stable_snapshot`,
  `screenshot`, `press`, `load`/`save`, `running` as the phase oracle) so a
  session writes `p.running(addr)` instead of re-deriving the two-read
  settle-and-compare each time. Module-level `diff`/`static_diff`/
  `intersect_changes` are the pure byte-diffing kernels the Protocols below
  name; `lua_macro(seq)` emits the menu-macro Lua source. Prefer it over
  re-typing a fresh heredoc; drop to raw `McpClient` calls for anything
  game-specific it doesn't cover.

## The laws (each one was paid for)

1. **Verify the PHASE before interpreting reads.** Frames advancing? Really in a fight?
   `game.controllable()` plus a free-running byte (sub-second timer) as the running/paused
   oracle (`Probe.running(oracle_addr)`) — in-game pause can be invisible to the gate until
   a pause flag is mapped.
2. **Write-tests beat correlation.** A candidate is only verified when writing it produces
   the predicted observable (teleport, label re-render, damage, driver decode). A write that
   "reverts" may be the written value being CONSUMED (MK2 Genesis timer) — check what the
   post-write value trajectory means before declaring disproof.
3. **Save states are checkpoints.** Get into the interesting phase ONCE, `save_state`, then
   every experiment starts from `load_state`. Deterministic replay turns diffing into exact
   set-intersection.
4. **Everything sequential is ONE script.** Real time runs between your turns; menus time out.

## Protocols

- **Static-diff**: snapshot a region twice per state (keep only bytes stable in both),
  diff stable-vs-stable across states. Finds config/flags; prunes animation noise.
  `Probe.stable_snapshot()` takes the two-per-state pair; `re.static_diff(pair_a, pair_b)`
  does the stable-in-both-AND-differing diff across states.
- **Toggle-intersect**: same screen, cycle a setting N times, intersect the changed-sets.
  The setting byte survives; frame counters don't. Beware DERIVED echoes (rendered-label
  bytes) — only a write-test that provokes re-derivation identifies the source.
  `re.intersect_changes([snap0, snap1, ...])` returns offsets that changed at every step.
- **Controlled-motion intersect**: for positions, let the CPU walk (or inject a held
  direction), pause between snapshots, require monotone change in the walk direction.
  `Probe.snapshot()` + `re.diff(a, b)` for the pairwise read.
- **Tick-boundary intersect**: for timers/counters, diff exactly across the visible tick.
  Same `re.diff` kernel, timed to straddle the tick.
- **In-engine menu macros** (beats MCP round-trip latency every time): frame-scheduled
  input via Lua — `re.lua_macro(seq, port=0)` builds the source below from
  `seq = [(at_frame, mask_or_button_names, hold_frames)]` (names or raw masks; see
  `re.BUTTON_MASKS`) and returns it as a string to hand to `c.call("run_lua", script=...)`:
  ```lua
  local seq = {{at=5, mask=0x8, hold=3}, {at=90, mask=0x20, hold=3}}  -- start, down
  local f0 = emu.framecount(); local done = false
  event.onframeend(function()
    if done then return end
    local f = emu.framecount() - f0
    for _, s in ipairs(seq) do if f >= s.at and f < s.at + s.hold then input.set(0, s.mask) end end
    if f > 200 then done = true end
  end)
  ```
  Masks: b=0x1 y=0x2 select=0x4 start=0x8 up=0x10 down=0x20 left=0x40 right=0x80 a=0x100 x=0x200 l=0x400 r=0x800.
- **Screenshots as eyes**: `c.screenshot(path)` (or `Probe.screenshot(path)`, which pauses
  first for a stable frame automatically), then view the PNG. Menus often need a
  blink/fade allowance — step a few frames and reshoot.

## Known platform quirks

- **`press_buttons(frames=N)` does NOT reliably sustain a HELD input.** A
  single long call (e.g. frames=180) fails game logic that requires a
  continuous hold — guard checks especially. Proven 3/3 by A/B: only
  repeated short re-presses (~every 40 ms) or the native `DummyMode::Block`
  sustain it. This ALREADY caused one wrong conclusion (a "contact signal"
  that was really the guard being dropped and re-entered, generating action
  transitions). If an experiment depends on something being held, verify the
  hold is actually live before trusting the result.

- `freeze` does not land on direct-pointer regions (FBNeo fallback RAM) — periodic writes
  (or a profile `pin`) are the mechanism.
- Injected input drains while paused (pause→step loses it) — inject only while running.
- Genesis on FBNeo: WRAM at 0xFF0000, community FFxxxx addresses map directly.
  TMS34010 (arcade MK2): blob_offset = (bit_addr − 0x01000000) >> 3.

## Exit criteria

Findings go to `library/<game>/<game or port>.md` (evidence: method, values, date) and the
profile JSON — never into code. Record DISPROVEN candidates and research traps too; they
save the next session. Kill your instance when done.
