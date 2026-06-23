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
  cargo run --release -- \
    --core ./genesis_plus_gx_libretro.dylib \
    --rom ./game.md \
    --debug \
    --script ./examples/hitbox_demo.lua
  ```

- **At runtime**, press **F10** to open the floating **Lua Script** window. Type a
  path (or inline Lua) into **Script path:** and click **Load**. **Reload** hot-reloads
  from a fresh VM; **Clear VM** discards everything. The status line confirms how many
  `onframeend` callbacks registered.

## The v1 API

Scripts run in a sandbox — only `table`, `string`, and `math` stdlibs; no `io`, `os`,
or `package`, so a script can't touch your filesystem (the *host* reads the file, not
Lua). Available globals (check `_RUSTRETRO_API >= 1` to feature-detect):

```text
memory.read_u8(addr)        memory.read_u16_be(addr)    memory.read_u32_be(addr)
memory.read_s16_be(addr)    memory.read_u16_le(addr)    memory.read_u32_le(addr)
gui.drawBox(x1,y1,x2,y2, fill, line)     gui.drawText(x,y, str [, color])
gui.drawLine(x1,y1,x2,y2, color)         gui.drawPixel(x,y, color)
event.onframeend(function)               console.log(str)        emu.framecount()
```

Colors are packed `0xRRGGBBAA` (`AA=0xFF` is opaque). Coordinates are in **game-pixel
space** (e.g. 320×224), 1:1 with the framebuffer before upscaling. Genesis is
big-endian — reach for the `_be` reads.

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
those object-RAM addresses with [RAM Search](ram-search.md) and
[Tracking Changes](tracking-changes.md), read pixel coordinates with the
[Frame Inspector](tiles-and-frames.md) picker, then:

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

- [RAM Search](ram-search.md) / [Tracking Changes](tracking-changes.md) — find the object-RAM addresses to read.
- [Tiles & Frames](tiles-and-frames.md) — pixel-pick coordinates in the same game-pixel space your script draws in.

<!-- litui:live
When litui is integrated, this page gains live embeds:
- [custom](script_output_slot) a live script-output / console.log readout (the 🧾 Log lines a script emits) — escape hatch
- [custom](script_editor_slot) the F10 Script window's path field + Load / Reload / Clear VM controls (escape hatch)
- [textarea] for inline Lua and a [display] of the "N onframeend callbacks registered" status (live-resource binding)
Until then it renders as a static document page.
-->
