-- hitbox_demo.lua — RustRetro v1 Lua overlay template.
--
-- This is a minimal end-to-end smoke test AND a starting template for real
-- fighting-game hitbox-overlay scripts. It draws one translucent box every frame
-- and logs a memory read, proving the pipeline works even without a real game.
--
-- API available to scripts (v1):
--   memory.read_u8(addr)              -> integer
--   memory.read_u16_be(addr)          -> integer  (big-endian; Genesis is BE!)
--   memory.read_u32_be(addr)          -> integer
--   memory.read_s16_be(addr)          -> integer  (signed)
--   gui.drawBox(x1,y1,x2,y2, fill, line)          colors are 0xRRGGBBAA
--   gui.drawText(x,y, str [, color])
--   event.onframeend(function)        register a per-frame callback
--   console.log(str)                  write to the debug event log
--
-- Coordinates are in GAME-PIXEL space (e.g. 320x224 for Genesis), NOT window
-- pixels — they line up 1:1 with the emulated framebuffer before upscaling.

local frame = 0

event.onframeend(function()
  frame = frame + 1

  -- Translucent green fill (alpha 0x60) with a solid green 1px outline.
  -- A real hitbox script reads box coords from object RAM instead of hardcoding.
  gui.drawBox(50, 50, 100, 100, 0x00FF0060, 0x00FF00FF)

  -- A small label near the box.
  gui.drawText(50, 40, "HITBOX", 0xFFFFFFFF)

  -- Example big-endian reads. On a real Genesis game these would be object-RAM
  -- addresses holding box edges; here we just read some RAM and log it once a
  -- second so the console isn't spammed.
  if frame % 60 == 0 then
    local v = memory.read_u16_be(0xFF0000)  -- start of Genesis work RAM
    console.log(string.format("frame %d: word@FF0000 = 0x%04X", frame, v))
  end

  -- ── Real hitbox template (commented out) ──────────────────────────────────
  -- local count = memory.read_u8(0xFFB000)        -- number of active boxes
  -- for i = 0, count - 1 do
  --   local base = 0xFFB010 + i * 8
  --   local x1 = memory.read_s16_be(base + 0)
  --   local y1 = memory.read_s16_be(base + 2)
  --   local x2 = memory.read_s16_be(base + 4)
  --   local y2 = memory.read_s16_be(base + 6)
  --   gui.drawBox(x1, y1, x2, y2, 0xFF000040, 0xFF0000FF)  -- red attack box
  -- end
end)

console.log("hitbox_demo.lua loaded")
