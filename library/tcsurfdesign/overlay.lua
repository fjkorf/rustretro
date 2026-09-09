-- T&C Surf Designs (NES) — live-detail overlay.
--
-- Load:  --game library/tcsurfdesign --script library/tcsurfdesign/overlay.lua
--
-- Draws player position/velocity/state, controller input, mode/screen state,
-- camera scroll, and physics aids on top of the game's own 256x240 frame.
-- Every game-specific value is read through game.addr(NAME) — the profile's
-- globals map is the ONLY source of addresses (no raw addresses law). A field
-- whose global is absent renders "--" (never a fake 0: absent ≠ zero; the
-- engine's read path folds unmapped reads into 0, so the nil from game.addr
-- is the one honest absent-signal this script gets).
--
-- CONFIG is a GLOBAL table on purpose: MCP run_lua evaluates chunks that can
-- only reach _G, so `run_lua("CONFIG.sections.physics = false")` retunes the
-- overlay live with no reload. F10 Reload (or a fresh --script launch)
-- resets every toggle to the file defaults below — reload builds a new VM.
--
-- Single-actor game: game.block1/block2/field_off and training.* are
-- fighting-game vocabulary with no referent here and are deliberately unused.

if not _RUSTRETRO_API or _RUSTRETRO_API < 3 then
  error("tcsurfdesign overlay needs RustRetro Lua API v3+")
end

CONFIG = {
  sections = {
    position = true,
    physics  = true,
    input    = true,
    mode     = true,
    camera   = true,
    gate     = false,   -- off: may be vacuously wrong until the profile gains a gate
    physics_derived_ground = false, -- judgment-call ground marker (needs GROUND_Y calibration)
  },
  -- The ONE place game-specific read shape lives. Keys must be byte-identical
  -- to the profile's memory.globals names (game.addr is an exact string
  -- lookup). Widths/signs come from the evidence doc; "s8" is sign-extended
  -- here (the engine has no read_s8 binding).
  fields = {
    player_x     = { reader = "u8",     label = "X" },
    player_y     = { reader = "u8",     label = "Y" },
    player_vx    = { reader = "s8",     label = "VX" },
    player_vy    = { reader = "s8",     label = "VY" },
    player_state = { reader = "u8",     label = "ST" },
    on_ground    = { reader = "u8",     label = "GND" },
    game_mode    = { reader = "u8",     label = "MODE" },
    camera_x     = { reader = "u16_le", label = "CAMX" },
    input_p1     = { reader = "u8",     label = "IN" },
  },
  mode_names = {},   -- populated as the evidence doc confirms values, e.g. [0]="TITLE"
  layout = {
    pos_panel  = { x = 2, y = 2 },
    mode_panel = { x = 196, y = 2 },
    input_y    = 231,
    vector_scale = 3,
  },
  colors = {
    ok      = 0xE8E8D0FF,
    dim     = 0x707078FF,
    warn    = 0xE0A040FF,
    derived = 0x60A0E080,
    panel   = 0x10102080,
    frame   = 0x404050FF,
    vec     = 0x60E060FF,
    gnd_on  = 0x60E060FF,
    gnd_off = 0xE06060FF,
  },
}

-- ── engine (game-agnostic below this line) ──────────────────────────────────

local function read_field(name)
  local a = game.addr(name)
  if a == nil then return nil end
  local f = CONFIG.fields[name]
  local r = f and f.reader or "u8"
  if r == "u8" then return memory.read_u8(a) end
  if r == "s8" then
    local v = memory.read_u8(a)
    if v >= 0x80 then v = v - 0x100 end
    return v
  end
  if r == "u16_le" then return memory.read_u16_le(a) end
  if r == "u16_be" then return memory.read_u16_be(a) end
  if r == "s16_be" then return memory.read_s16_be(a) end
  return memory.read_u8(a)
end

local function fmt(name)
  local v = read_field(name)
  if v == nil then return "--", CONFIG.colors.dim end
  return tostring(v), CONFIG.colors.ok
end

local function draw_panel(x, y, w, lines)
  local h = 2 + #lines * 8
  gui.drawBox(x, y, x + w, y + h, CONFIG.colors.panel, CONFIG.colors.frame)
  for i, ln in ipairs(lines) do
    gui.text(x + 2, y + 2 + (i - 1) * 8, ln.text, ln.color or CONFIG.colors.ok)
  end
end

-- One section erroring must not blank the rest of the frame's overlay.
local function safe_section(name, fn)
  local ok, err = pcall(fn)
  if not ok then
    gui.text(2, 220, "ERR:" .. name, CONFIG.colors.warn)
    if not _ERR_LOGGED then _ERR_LOGGED = {} end
    if not _ERR_LOGGED[name] then
      _ERR_LOGGED[name] = true
      console.log("overlay section '" .. name .. "' error: " .. tostring(err))
    end
  end
end

-- ── sections ────────────────────────────────────────────────────────────────

local function line(name)
  local f = CONFIG.fields[name]
  local txt, col = fmt(name)
  return { text = (f and f.label or name) .. " " .. txt, color = col }
end

local function draw_position()
  local L = CONFIG.layout.pos_panel
  draw_panel(L.x, L.y, 52, { line("player_x"), line("player_y"), line("player_state") })
end

local function draw_physics()
  local vx, vy = read_field("player_vx"), read_field("player_vy")
  local px, py = read_field("player_x"), read_field("player_y")
  local L = CONFIG.layout.pos_panel
  draw_panel(L.x, L.y + 30, 52, { line("player_vx"), line("player_vy"), line("on_ground") })
  -- Velocity vector, world-anchored: only when position AND velocity are both
  -- present — never anchor a vector on a missing position.
  if vx and vy and px and py then
    local s = CONFIG.layout.vector_scale
    gui.drawLine(px, py, px + vx * s, py + vy * s, CONFIG.colors.vec)
  end
  local gnd = read_field("on_ground")
  if gnd ~= nil and px and py then
    gui.drawPixel(px, py + 2, gnd ~= 0 and CONFIG.colors.gnd_on or CONFIG.colors.gnd_off)
  elseif CONFIG.sections.physics_derived_ground and px and py then
    local gy = game.calibration("GROUND_Y")
    if gy then
      -- HOLLOW derived marker: a judgment call, visually distinct from a
      -- measured ground flag; off by default until GROUND_Y is confirmed.
      local c = (py >= gy) and CONFIG.colors.derived or CONFIG.colors.dim
      gui.drawBox(px - 2, py + 1, px + 2, py + 3, 0x00000000, c)
    end
  end
end

-- RETRO joypad bit order (engine convention, not a guest fact).
local IN_BITS = {
  { 4, "U" }, { 5, "D" }, { 6, "L" }, { 7, "R" },
  { 2, "s" }, { 3, "S" }, { 0, "B" }, { 8, "A" },
}

local function draw_input()
  local m = input.get(0)  -- engine primitive: post-fold asserted input, always available
  local parts = {}
  for _, b in ipairs(IN_BITS) do
    local bit, ch = b[1], b[2]
    local on = math.floor(m / 2 ^ bit) % 2 == 1
    parts[#parts + 1] = on and ch or "."
  end
  local txt = "P1 " .. table.concat(parts)
  local refl = read_field("input_p1")
  if refl ~= nil then txt = txt .. string.format("  ram:%02X", refl) end
  gui.text(2, CONFIG.layout.input_y, txt, CONFIG.colors.ok)
end

local function draw_mode()
  local L = CONFIG.layout.mode_panel
  local v = read_field("game_mode")
  local txt, col
  if v == nil then
    txt, col = "MODE --", CONFIG.colors.dim
  else
    txt, col = (CONFIG.mode_names[v] or ("MODE " .. v)), CONFIG.colors.ok
  end
  draw_panel(L.x, L.y, 58, { { text = txt, color = col } })
end

local function draw_camera()
  local L = CONFIG.layout.mode_panel
  draw_panel(L.x, L.y + 14, 58, { line("camera_x") })
end

local function draw_gate()
  -- Labeled caveat baked in: an empty profile gate reads CLOSED (engine-side
  -- fix), but a wrong/partial gate can still mislead — debug corner only.
  local c = game.controllable()
  gui.text(196, 224, "gate:" .. tostring(c), CONFIG.colors.dim)
end

event.onframeend(function()
  if CONFIG.sections.position then safe_section("position", draw_position) end
  if CONFIG.sections.physics  then safe_section("physics",  draw_physics)  end
  if CONFIG.sections.input    then safe_section("input",    draw_input)    end
  if CONFIG.sections.mode     then safe_section("mode",     draw_mode)     end
  if CONFIG.sections.camera   then safe_section("camera",   draw_camera)   end
  if CONFIG.sections.gate     then safe_section("gate",     draw_gate)     end
end)

-- Load-time summary: one log line per field, so the debug log says instantly
-- which fields are live vs still awaiting the profile's globals.
for name, _ in pairs(CONFIG.fields) do
  local a = game.addr(name)
  if a then
    console.log(string.format("overlay field %-12s FOUND  @0x%04X", name, a))
  else
    console.log(string.format("overlay field %-12s MISSING (no profile global)", name))
  end
end
console.log("tcsurfdesign overlay loaded (API v" .. tostring(_RUSTRETRO_API) .. ")")
