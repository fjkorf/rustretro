-- cps2_oam_probe.lua — RustRetro AI Wave 2 sprite/OAM probe TEMPLATE.
--
-- PURPOSE
--   Enumerate the on-screen sprites of a CPS2 game by walking its OBJECT RAM
--   table and logging each entry (X, Y, tile#, attributes). This is the first
--   step of the conversational "which ROM holds the on-screen sprite pieces?"
--   workflow:
--       1. (this script) walk object RAM  -> tile numbers + positions
--       2. read_region("VRAM"/gfx, ...)   -> the raw tile bytes
--       3. vram_to_rom / search_memory     -> candidate ROM addresses
--
-- HONEST SCOPE — READ THIS
--   There is NO universal sprite decoder. CPS2 object RAM is NOT the same as
--   Genesis VDP sprites or NES OAM. The constants below are PLACEHOLDERS. You
--   (or Claude, reasoning over app://state + read_region dumps) must fill them
--   in for the specific game/driver loaded. Treat this file as scaffolding that
--   proves the run_lua -> enumerate -> overlay path end to end, not as a
--   plug-and-play decoder.
--
-- HOW CPS2 OBJECT RAM IS LAID OUT (general shape — verify per game/driver)
--   CPS2 maintains an "object RAM" (sprite list) that the hardware DMAs to the
--   sprite chip each frame. It is a flat array of fixed-stride entries. A common
--   shape per entry is 8 bytes / 4 words (big-endian; the 68000 is BE):
--       word 0 (off 0): X position        (often only low 10 bits used)
--       word 1 (off 2): Y position
--       word 2 (off 4): tile number / code (index into the GFX ROM tile space)
--       word 3 (off 6): attributes         (palette, X/Y flip, size/zoom select)
--   The list is usually terminated by a sentinel entry (e.g. an all-zero or a
--   specific end-marker word) or has a fixed maximum length. EXACT base address,
--   stride, field offsets, bit masks, and the GFX-ROM tile stride are
--   game/driver-specific — fbalpha2012 and other cores differ. Find the base by:
--     * reading app://memory-map to see which region is object/sprite RAM, then
--     * read_region() that region and look for a table whose word-2 values track
--       what's visibly on screen as sprites move.
--
-- API (see examples/hitbox_demo.lua for the full v1 surface):
--   memory.read_u8/read_u16_be/read_u32_be/read_s16_be(addr)
--   gui.drawBox(x1,y1,x2,y2, fill_rgba, line_rgba)   colors 0xRRGGBBAA
--   gui.drawText(x,y, str [, color]);  console.log(str)
--   event.onframeend(function)

-- ── FILL THESE IN PER GAME/DRIVER (placeholders!) ───────────────────────────
local OBJ_BASE     = 0xFF0000   -- absolute guest addr of the object-RAM table
local ENTRY_STRIDE = 8          -- bytes between consecutive sprite entries
local MAX_ENTRIES  = 256        -- table length / scan cap (avoid runaway loops)

local OFF_X    = 0              -- byte offset of X word within an entry
local OFF_Y    = 2              -- byte offset of Y word
local OFF_TILE = 4              -- byte offset of tile# word
local OFF_ATTR = 6              -- byte offset of attribute word

local X_MASK     = 0x03FF       -- mask off non-position bits from the X word
local Y_MASK     = 0x03FF
local FLIP_X_BIT = 0x2000       -- example attribute bits — VERIFY per driver
local FLIP_Y_BIT = 0x4000

-- Stop scanning when we hit this many consecutive "empty" entries (tile==0).
local EMPTY_RUN_STOP = 8

-- Set true to also outline each enumerated sprite on the framebuffer. Proves the
-- overlay path; positions/sizes are approximate until you confirm the layout.
local DRAW_OVERLAY = true
local TILE_PX = 16              -- assumed sprite cell size for the overlay box

-- ── enumerate one frame's worth of sprites ──────────────────────────────────
local function read_entry(i)
  local base = OBJ_BASE + i * ENTRY_STRIDE
  local x    = memory.read_u16_be(base + OFF_X)    & X_MASK
  local y    = memory.read_u16_be(base + OFF_Y)    & Y_MASK
  local tile = memory.read_u16_be(base + OFF_TILE)
  local attr = memory.read_u16_be(base + OFF_ATTR)
  return x, y, tile, attr
end

local function scan_sprites()
  local found = 0
  local empty_run = 0
  for i = 0, MAX_ENTRIES - 1 do
    local x, y, tile, attr = read_entry(i)

    -- Heuristic "is this a live sprite?" — adapt to the game's sentinel rule.
    if tile == 0 then
      empty_run = empty_run + 1
      if empty_run >= EMPTY_RUN_STOP then break end
    else
      empty_run = 0
      found = found + 1
      local fx = (attr & FLIP_X_BIT) ~= 0
      local fy = (attr & FLIP_Y_BIT) ~= 0
      console.log(string.format(
        "obj[%3d] X=%4d Y=%4d tile=0x%04X attr=0x%04X flipX=%s flipY=%s",
        i, x, y, tile, attr, tostring(fx), tostring(fy)))

      if DRAW_OVERLAY then
        -- Outline the sprite cell. Coords are GAME-PIXEL space (pre-upscale).
        gui.drawBox(x, y, x + TILE_PX, y + TILE_PX, 0x00FFFF20, 0x00FFFFFF)
        gui.drawText(x, y - 8, string.format("0x%04X", tile), 0xFFFF00FF)
      end
    end
  end
  return found
end

-- Run once immediately so a single run_lua call returns enumerated entries in
-- its console output (the MCP round-trip captures console.log).
local n = scan_sprites()
console.log(string.format("cps2_oam_probe: enumerated %d sprite entries at base 0x%X (FILL IN CONSTANTS!)", n, OBJ_BASE))

-- Also register a per-frame callback so the overlay keeps tracking while the
-- game runs (useful when driving the app interactively, not just one-shot).
if DRAW_OVERLAY then
  event.onframeend(function() scan_sprites() end)
end
