---
page:
  name: LuaScripting
  label: "Lua Scripting"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# Lua Scripting — Build a Hitbox Overlay

**What you'll do:** load a Lua script that reads object RAM and draws translucent boxes
onto the live framebuffer — the killer use case being fighting-game hitbox overlays.

## Load a script

There are two ways in:

- **At launch**, with the `--script` flag:

  ```bash
  ./target/release-dev/rustretro \
    --core ./genesis_plus_gx_libretro.dylib \
    --rom ./game.md \
    --debug \
    --script ./examples/hitbox_demo.lua
  ```

- **At runtime**, press **F10** to open the floating **Lua Script** window. Type a
  path (or inline Lua) into **Script path:** and click **Load**. **Reload** hot-reloads
  from a fresh VM; **Clear VM** discards everything. The status line confirms how many
  `onframeend` callbacks registered.

## The API

Scripts run in a sandbox — only `table`, `string`, and `math` stdlibs; no `io`, `os`,
or `package`, so a script can't touch your filesystem (the *host* reads the file, not
Lua). Available globals (check `_RUSTRETRO_API >= 2` to feature-detect):

```text
memory.read_u8(addr)        memory.read_u16_be(addr)    memory.read_u32_be(addr)
memory.read_s16_be(addr)    memory.read_u16_le(addr)    memory.read_u32_le(addr)
memory.writebyte(addr, v)   memory.writeword(addr, v)   -- gated, see below
savestate.save(slot_or_path)             savestate.load(slot_or_path)
input.set(port, mask_or_table)           input.get(port)
gui.drawBox(x1,y1,x2,y2, fill, line)     gui.drawText(x,y, str [, color [, scale]])
gui.text(x,y, str [, color [, scale]])   -- drawText + 1px drop shadow
gui.drawLine(x1,y1,x2,y2, color)         gui.drawPixel(x,y, color)
event.onframeend(function)               console.log(str)        emu.framecount()
```

Colors are packed `0xRRGGBBAA` (`AA=0xFF` is opaque). Coordinates are in **game-pixel
space** (e.g. 320×224), 1:1 with the framebuffer before upscaling. Genesis is
big-endian — reach for the `_be` reads. The `writebyte`/`savestate`/`gui.text` names
follow the FBNeo/FBA Lua conventions, so scripts from that ecosystem port naturally.

### Memory writes (gated)

`memory.writebyte(addr, v)` pokes one byte; `memory.writeword(addr, v)` pokes a
16-bit word in **guest big-endian** order (high byte at `addr`, mirroring
`read_u16_be` — `writeword(a, v)` then `read_u16_be(a)` round-trips). Writes route
through the same path as the debugger's `write_memory`: bus-window addresses are
pushed to the live 68k bus on the next frame drain.

Writes are **off by default**: they raise an error naming the `lua_writes_enabled`
gate. Launch with `--training` to arm them, or arm/lock at runtime with the MCP
`enable_writes` / `disable_writes` tools.

### Save states (queued)

`savestate.save(3)` / `savestate.load(3)` use numbered slots 1-9
(`<save_dir>/<rom>.stateN`); pass a string for an explicit file path. Ops are
**queued** — applied on the next frame by the emulation thread — and return `true`
when enqueued; if another op is still in flight you get an error (retry next
frame). Loading is *not* behind the write gate: scripts are user-authored, and a
state load carries the same trust as the save-state hotkeys.

### Input injection

`input.set(port, spec)` drives controller port 0 (P1) or 1 (P2). `spec` is either a
12-bit mask (bit *i* = RETRO id *i*: 0=B 1=Y 2=Select 3=Start 4=Up 5=Down 6=Left
7=Right 8=A 9=X 10=L 11=R) or a table of RETRO button names:

```lua
event.onframeend(function()
  input.set(1, {right=true, b=true})   -- P2 walks right, mashing B
end)
```

Each call holds the named buttons for **2 frames** and releases everything else, so
a per-frame callback re-asserting a button reads as continuously held, and dropping
it releases within 2 frames. `input.get(port)` returns the port's current 12-bit
mask (the state fed to the core on the last input fold — keyboard/pad OR injected).

### Legible labels

`gui.text` is `gui.drawText` plus a 1px black drop shadow, for HUD labels that stay
readable over bright game art.

## Walk through `examples/hitbox_demo.lua`

The shipped template proves the whole pipeline end-to-end:

1. It registers a per-frame callback with `event.onframeend(function() ... end)`.
2. Each frame it draws one translucent green box and a `"HITBOX"` label:

   ```lua
   gui.drawBox(50, 50, 100, 100, 0x00FF0060, 0x00FF00FF)
   gui.drawText(50, 40, "HITBOX", 0xFFFFFFFF)
   ```

3. Once a second it does a big-endian read and logs it (visible in the **🧾 Log** tab):

   ```lua
   if frame % 60 == 0 then
     local v = memory.read_u16_be(0xFF0000)  -- start of Genesis work RAM
     console.log(string.format("frame %d: word@FF0000 = 0x%04X", frame, v))
   end
   ```

## From template to a real overlay

The commented block at the bottom of the demo is the real shape: read a box count from
object RAM, loop, read each box's edges with `memory.read_s16_be`, and draw it. Find
those object-RAM addresses with [RAM Search](/docs/tutorials/ram-search.md) and
[Tracking Changes](/docs/tutorials/tracking-changes.md), read pixel coordinates with the
[Frame Inspector](/docs/tutorials/tiles-and-frames.md) picker, then:

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

A buggy script never crashes the app — callback errors are caught and logged, and you
just **Reload**.

## Why it matters

Hitbox overlays are *the* tool fighting-game players reverse a game to build. Reading
object RAM and compositing boxes onto the exact frame — without rebuilding a core — is
what this whole instrument is for.

## See also

- [RAM Search](/docs/tutorials/ram-search.md) / [Tracking Changes](/docs/tutorials/tracking-changes.md) — find the object-RAM addresses to read.
- [Tiles & Frames](/docs/tutorials/tiles-and-frames.md) — pixel-pick coordinates in the same game-pixel space your script draws in.

<!-- litui:live
When litui is integrated, this page gains live embeds:
- [custom](script_output_slot) a live script-output / console.log readout (the 🧾 Log lines a script emits) — escape hatch
- [custom](script_editor_slot) the F10 Script window's path field + Load / Reload / Clear VM controls (escape hatch)
- [textarea] for inline Lua and a [display] of the "N onframeend callbacks registered" status (live-resource binding)
Until then it renders as a static document page.
-->
