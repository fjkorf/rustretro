---
page:
  name: LuaScripting
  label: "Lua Scripting"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# Lua Scripting — API v3

**What you'll do:** load a Lua script that reads game memory and draws overlays onto
the live framebuffer — from a minimal hitbox demo up to `training.lua`, the full
worked example of the v3 profile-boundary contract.

## Load a script

- **At launch**, with `--script`:

  ```bash
  ./target/release-dev/rustretro \
    --core ./genesis_plus_gx_libretro.dylib \
    --rom ./game.md \
    --debug \
    --script ./examples/hitbox_demo.lua
  ```

- **At runtime**, press **F10** to open the floating **Lua Script** window. Type a path
  (or inline Lua) into **Script path:** and click **Load**. **Reload** hot-reloads from
  a fresh VM — this destroys all VM state, including any in-memory buffers a script
  built up — and **Clear VM** discards everything.

## The sandbox

Scripts run with only the `table`, `string`, and `math` stdlibs — no `io`, `os`, or
`package`. A script can never touch your filesystem directly (the *host* reads the
script file, not Lua) and can never persist data across a reload on its own. Feature
-detect the API with `_RUSTRETRO_API` (currently `3`):

```lua
if not _RUSTRETRO_API or _RUSTRETRO_API < 3 then
  error("this script needs API v3+")
end
```

## The full API (v3)

```text
memory.read_u8(addr)              memory.read_u16_be(addr)     memory.read_u32_be(addr)
memory.read_s16_be(addr)          memory.read_u16_le(addr)     memory.read_u32_le(addr)
memory.writebyte(addr, v)         memory.writeword(addr, v)    -- gated
memory.freeze(addr, v)            memory.unfreeze(addr)        -- gated
savestate.save(slot_or_path)      savestate.load(slot_or_path) -- queued
input.set(port, mask_or_table)    input.get(port)
gui.drawBox(x1,y1,x2,y2, fill, line)
gui.drawText(x,y, str [, color [, scale]])   gui.text(x,y, str [, color [, scale]])
gui.drawLine(x1,y1,x2,y2, color)  gui.drawPixel(x,y, color)
event.onframeend(function)        console.log(str)
emu.framecount()                  emu.paused()
game.controllable()               game.addr(name)
game.block1() / game.block2()     game.field_off(name)
game.char_name(id)                game.matchup_slug(me, opp)
game.stage_value_for(opp)         game.calibration(key)
training.enabled()                training.refill()            training.dummy()
shadow.on()                       shadow.model()                shadow.toggle()
record.active()                   record.path()                 record.frames()
record.start(path [, style])      record.stop()                 -- queued
_RUSTRETRO_API = 3
```

Colors are packed `0xRRGGBBAA` (`AA=0xFF` opaque). Coordinates are game-pixel space,
1:1 with the framebuffer before upscaling. `writebyte`/`savestate`/`input`/`gui.text`
follow FBNeo/FBA Lua conventions, so scripts from that ecosystem port naturally.
`memory.read_u16_be`/`read_u32_be`/`read_s16_be` are big-endian (Genesis, 68k games);
`_le` variants exist for little-endian machines.

### The write gate

`memory.writebyte`/`writeword`/`freeze`/`unfreeze` all raise an error naming the
`lua_writes_enabled` gate unless it's armed — launch with `--training`, or arm/lock at
runtime with the MCP `enable_writes`/`disable_writes` tools. `freeze(addr, v)` installs
a standing per-frame write (the same mechanism the Watch panel's freeze checkbox and the
Matchup panel's force-matchup button use); `unfreeze(addr)` drops it. `savestate.load`
is **not** gated — scripts are user-authored, and loading a state carries the same trust
as the save-state hotkeys.

`memory.writeword(a, v)` writes guest (68k) big-endian order — the high byte at `addr`
— so it round-trips with `read_u16_be(a)`.

### The profile boundary — `game.*`/`training.*`/`shadow.*`/`record.*`

These four tables are the v3 design ruling made concrete: **logic lives once, in Rust;
Lua asks via bindings, it never re-implements.** `game.controllable()` evaluates the
loaded profile's gate condition list — the same evaluator the recorder uses — instead
of a script keeping its own copy. `game.addr(name)` / `game.block1()`/`block2()` /
`game.field_off(name)` resolve by NAME from the loaded profile
(`library/<game>/<game>.profile.json`); **scripts never hardcode raw addresses**. A game
port is a new profile, not a script rewrite. `training.*` reads the native
training-mode state (enforcement — credits/timer/health — is owned by native code, not
Lua); `shadow.*` reads/toggles the loaded shadow bot; `record.*` reads and drives the
native jsonl session recorder.

### Input injection

`input.set(port, spec)` drives port 0 (P1) or 1 (P2). `spec` is a 12-bit mask (bit *i* =
RETRO id *i*: 0=B 1=Y 2=Select 3=Start 4=Up 5=Down 6=Left 7=Right 8=A 9=X 10=L 11=R) or a
table of RETRO button names, e.g. `{right=true, b=true}`. Each call holds the named
buttons for 2 frames and releases everything else — a per-frame callback re-asserting a
button reads as continuously held. `input.get(port)` returns the port's current 12-bit
mask.

## Worked example: `examples/hitbox_demo.lua`

The shipped template proves the pipeline end to end: one `event.onframeend` callback
draws a translucent green box and a `"HITBOX"` label every frame, and once a second logs
a big-endian memory read to the **🧾 Log** tab. The commented block at the bottom shows
the real shape — read a box count from object RAM, loop, read each box's edges with
`memory.read_s16_be`, draw them:

```lua
local count = memory.read_u8(0xFFB000)
for i = 0, count - 1 do
  local base = 0xFFB010 + i * 8
  local x1 = memory.read_s16_be(base + 0)
  local y1 = memory.read_s16_be(base + 2)
  local x2 = memory.read_s16_be(base + 4)
  local y2 = memory.read_s16_be(base + 6)
  gui.drawBox(x1, y1, x2, y2, 0xFF000040, 0xFF0000FF)  -- red attack box
end
```

Find real object-RAM addresses with [RAM Search](/docs/tutorials/ram-search.md) and
[Tracking Changes](/docs/tutorials/tracking-changes.md); pixel-pick coordinates with the
[Frame Inspector](/docs/tutorials/tiles-and-frames.md).

## Worked example: `library/asurabld/training.lua`

The full v3 contract in practice — read its header comment for the design rationale.
It's loaded alongside native `--training` and adds ONLY what the engine's schema
doesn't cover: a scripted port-1 dummy (stand/crouch/jump/block/replay) with an
in-memory record/replay buffer, hitstun tracking, and a stat overlay reading
`game.*`/`training.*`/`shadow.*`/`record.*`. It deliberately does not touch enforcement
— credits, timer, and health refill stay native — so training mode behaves identically
whether or not the script is loaded. Because the sandbox has no `io`, its replay buffer
is in-memory only and a reload (F10 "Reload") discards it; to change a `CONFIG` field
without losing the buffer, mutate the live VM via the MCP `run_lua` tool instead of
reloading.

A buggy script never crashes the app — callback errors are caught and logged, and you
just **Reload**.

## Why it matters

Reading game memory and compositing overlays onto the exact frame — without rebuilding
a core, and without a script ever needing to know a raw address — is what makes the
v3 API a portable per-game surface instead of a per-game hack.

## See also

- [RAM Search](/docs/tutorials/ram-search.md) / [Tracking Changes](/docs/tutorials/tracking-changes.md) — find the addresses a script reads.
- [Training Mode](/docs/tutorials/training-mode.md) — the native state training.lua reads via `training.*`.
- [Porting a Game](/docs/tutorials/porting-a-game.md) — how `game.*` gets a new game's addresses without touching a script.

<!-- litui:live
When litui is integrated, this page gains live embeds:
- [custom](script_output_slot) a live script-output / console.log readout (the 🧾 Log lines a script emits) — escape hatch
- [custom](script_editor_slot) the F10 Script window's path field + Load / Reload / Clear VM controls (escape hatch)
- [textarea] for inline Lua and a [display] of the "N onframeend callbacks registered" status (live-resource binding)
Until then it renders as a static document page.
-->
