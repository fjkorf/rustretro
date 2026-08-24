-- shadow_overlay.lua — Wave-0 "shadow AI" TRAINING SURFACE for RustRetro.
--
-- A live, on-screen readout of both fighters' internal state for Asura Blade,
-- drawn straight onto the emulated framebuffer every frame. It is the Wave-0
-- ZERO-ENGINE-CODE training surface: it needs no changes to the emulator or the
-- core — it rides entirely on the existing sandboxed Lua scripting engine
-- (src/lua_engine.rs), exactly like frame_meter.lua and hitbox_demo.lua.
--
-- WHAT IT SHOWS, every frame:
--   * Two tidy stat blocks — P1 top-left, P2 top-right — each with:
--       X   : actor X position        (base+0x54, u16 big-endian)
--       Y   : actor Y position        (base+0x56, u16 big-endian)
--       ACT : current action/command  (base+0x50, u16 big-endian)
--   * A center strip: DISTANCE = |P1.X - P2.X| and which fighter is on the LEFT.
--   * P1's live INPUTS as a proxy: the movement-hold accumulators
--       R (base+0x28, u16) and L (base+0x2A, u16) plus the action index.
--     There is no documented gamepad-reflection address, so this is labeled
--     "(PROXY)" — it is honestly the hold-accumulators + action, NOT a decoded
--     pad state. Swap in a real pad-reflection address here when one is found.
--
-- HOW TO LOAD:
--   * At launch:      --script examples/shadow_overlay.lua
--   * While running:  the run_lua MCP tool, or the F10 script panel (Reload).
--
-- MEMORY MODEL: the actor bases below are BUS-WINDOW (guest) addresses declared
-- in library/asurabld/asurabld.busmap.json — Work RAM is windowed at guest
-- 0x400000 (len 0x10000). The bytes are guest byte order (big-endian), so all
-- multi-byte reads use the *_be variants. Out-of-map reads return 0 by design.
--
-- API used (v1): memory.read_u16_be, memory.read_u8, gui.drawBox,
-- gui.drawText(x,y,s,color,scale), event.onframeend, emu.framecount,
-- console.log. Colors are packed 0xRRGGBBAA. Coords are GAME-PIXEL space.
-- Sandbox note: no io/os/file access — this is display only.

-- ══════════════════════════════ CONFIG ═══════════════════════════════════════

-- Actor structure bases (guest / bus-window addresses).
local P1_BASE = 0x40454C
local P2_BASE = P1_BASE + 0xDB4   -- 0x405300

-- Field offsets within an actor struct.
local OFF_ACT   = 0x50   -- action / command index   (u16)
local OFF_X     = 0x54   -- X position               (u16)
local OFF_Y     = 0x56   -- Y position               (u16)
local OFF_RIGHT = 0x28   -- movement-hold: right     (u16 accumulator)
local OFF_LEFT  = 0x2A   -- movement-hold: left      (u16 accumulator)

-- Screen size (game-pixel space) used to anchor the P2 block to the right edge.
-- If the real resolution differs the block simply clips; adjust if needed.
local SCREEN_W = 320

-- Colors (0xRRGGBBAA).
local C_BG     = 0x0A0A12C0   -- translucent dark panel fill
local C_EDGE   = 0xFFFFFF40   -- faint panel outline
local C_TITLE1 = 0x60B0FFFF   -- P1 accent (blue)
local C_TITLE2 = 0xFF7060FF   -- P2 accent (red)
local C_TEXT   = 0xFFFFFFFF   -- stat text (white)
local C_DIM    = 0xB0B0B0FF   -- secondary / proxy text
local C_DIST   = 0xFFE060FF   -- distance readout (amber)

-- Layout.
local PANEL_W  = 56          -- panel width  (px)
local ROW_H    = 7           -- px per text row
local MARGIN   = 4           -- screen-edge margin
local TOP_Y    = 26          -- below the health-bar HUD at the very top

-- ══════════════════════════════ ENGINE ══════════════════════════════════════
-- (game-agnostic below here)

-- Read one actor's live state into a table. All reads are big-endian; the bus
-- returns 0 for unmapped/inactive regions, which we treat as "no data".
local function read_actor(base)
  return {
    act   = memory.read_u16_be(base + OFF_ACT),
    x     = memory.read_u16_be(base + OFF_X),
    y     = memory.read_u16_be(base + OFF_Y),
    right = memory.read_u16_be(base + OFF_RIGHT),
    left  = memory.read_u16_be(base + OFF_LEFT),
  }
end

-- A fight is considered "active" only if there is some non-zero position/action
-- signal. This guards against drawing garbage (and against spamming) when no
-- match is running and every read comes back 0.
local function fight_active(p1, p2)
  return (p1.x ~= 0 or p1.y ~= 0 or p1.act ~= 0
       or p2.x ~= 0 or p2.y ~= 0 or p2.act ~= 0)
end

-- Draw a filled, faintly-outlined panel with a colored title, then return the
-- y of the first content row (just under the title).
local function draw_panel(x, y, title, title_color, lines)
  local h = ROW_H * (1 + #lines) + 3
  gui.drawBox(x, y, x + PANEL_W, y + h, C_BG, C_EDGE)
  gui.drawText(x + 3, y + 2, title, title_color, 1)
  local cy = y + 2 + ROW_H
  for _, ln in ipairs(lines) do
    gui.drawText(x + 3, cy, ln.s, ln.c or C_TEXT, 1)
    cy = cy + ROW_H
  end
end

-- One-time notice throttle so the "no fight" path never spams the console.
local warned_idle = false

local function draw_overlay()
  local p1 = read_actor(P1_BASE)
  local p2 = read_actor(P2_BASE)

  if not fight_active(p1, p2) then
    -- Idle: one small, quiet marker so the surface is visibly loaded, and a
    -- single console note (not per-frame).
    gui.drawText(MARGIN, TOP_Y, "SHADOW OVERLAY - NO FIGHT", C_DIM, 1)
    if not warned_idle then
      console.log("shadow_overlay: no active fight (reads are 0) - waiting")
      warned_idle = true
    end
    return
  end
  warned_idle = false

  -- ── P1 block (top-left) ──────────────────────────────────────────────────
  -- Inputs are a PROXY: the movement-hold accumulators + action index, not a
  -- decoded pad. Labeled honestly.
  draw_panel(MARGIN, TOP_Y, "P1", C_TITLE1, {
    { s = string.format("X:%d", p1.x) },
    { s = string.format("Y:%d", p1.y) },
    { s = string.format("ACT:%d", p1.act) },
    { s = "IN (PROXY)", c = C_DIM },
    { s = string.format("R:%d L:%d", p1.right, p1.left), c = C_DIM },
  })

  -- ── P2 block (top-right) ─────────────────────────────────────────────────
  local p2x = SCREEN_W - MARGIN - PANEL_W
  draw_panel(p2x, TOP_Y, "P2", C_TITLE2, {
    { s = string.format("X:%d", p2.x) },
    { s = string.format("Y:%d", p2.y) },
    { s = string.format("ACT:%d", p2.act) },
  })

  -- ── Center strip: distance + who is on the left ──────────────────────────
  local dist = p1.x - p2.x
  if dist < 0 then dist = -dist end
  local side
  if p1.x < p2.x then
    side = "LEFT P1"
  elseif p2.x < p1.x then
    side = "LEFT P2"
  else
    side = "SAME X"
  end
  local cx = (SCREEN_W // 2) - 22
  gui.drawText(cx, TOP_Y + 2, string.format("DIST %d", dist), C_DIST, 1)
  gui.drawText(cx, TOP_Y + 2 + ROW_H, side, C_DIM, 1)
end

-- Register the per-frame hook. We draw every frame so the overlay stays visible
-- even while paused; the only frame-gated work is the idle console note above.
event.onframeend(function()
  draw_overlay()
end)

console.log(string.format(
  "shadow_overlay.lua loaded (Wave-0 training surface): P1@0x%06X P2@0x%06X",
  P1_BASE, P2_BASE))
