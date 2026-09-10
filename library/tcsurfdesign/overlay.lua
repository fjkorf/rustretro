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
    physics  = true,   -- derived velocity + confirmed ground flag
    input    = true,
    mode     = true,   -- mode name + timer
    gate     = true,   -- real gate since T1 (byte_nonzero $47)
  },
  -- The ONE place game-specific read shape lives. Keys must be byte-identical
  -- to the profile's memory.globals names (game.addr is an exact string
  -- lookup); names/widths come from tcsurfdesign.md's confirmed-globals
  -- table. "s8" is sign-extended here (the engine has no read_s8 binding).
  -- Fields with no confirmed global yet (velocity, state, ground, camera)
  -- stay listed and render "--" until the evidence doc confirms them.
  fields = {
    player_x_screen = { reader = "u8", label = "X" },
    player_y_screen = { reader = "u8", label = "Y" },
    action_state    = { reader = "u8", label = "ACT" },  -- $40A: 0 idle,1-4 dir,5 B,6 A/air (latches airborne)
    ground_air_flag = { reader = "u8", label = "GND" },  -- $428: 2=grounded 1=airborne
    mode_index      = { reader = "u8", label = "MODE" },
    char_select_latch = { reader = "u8", label = "CHR" }, -- $704: 0=A char, 1=B char
    lives_count     = { reader = "u8", label = "LIVES" }, -- $477: true lives (hearts misreport on wipeout)
    engine_clock    = { reader = "u8", label = "CLK" },   -- $04: freezes on pause
    time_sec_tens   = { reader = "u8", label = "T10s" },
    time_sec_ones   = { reader = "u8", label = "T1s" },
    time_tenths     = { reader = "u8", label = "T.1" },
    pad_latch_p1    = { reader = "u8", label = "IN" },
    -- Velocity is COMPUTED not stored (RE session #2) — no VX/VY byte exists;
    -- the physics section derives motion from player_x/y frame deltas instead.
  },
  -- mode_index values, live-verified (tcsurfdesign.md menu/flow map)
  mode_names = { [0] = "STREET SKATE", [1] = "BIG WAVE", [2] = "WOOD+WATER" },
  -- action_state decode (grounded = re-derived from input; airborne latches 6)
  action_names = { [0]="idle", [1]="right", [2]="left", [3]="down", [4]="up", [5]="B", [6]="A/air" },
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
  -- action_state decoded via action_names, else raw.
  local act = read_field("action_state")
  local act_txt, act_col = "ACT --", CONFIG.colors.dim
  if act ~= nil then
    act_txt = "ACT " .. (CONFIG.action_names[act] or tostring(act))
    act_col = CONFIG.colors.ok
  end
  draw_panel(L.x, L.y, 60, {
    line("player_x_screen"), line("player_y_screen"),
    { text = act_txt, color = act_col },
  })
end

-- Velocity is COMPUTED not stored (no VX/VY byte): DERIVE it from the
-- per-frame position delta, stored across frames in a global. Labeled
-- "(derived)" so it never reads as a measured field.
_PREV_POS = _PREV_POS or {}

local function draw_physics()
  local px, py = read_field("player_x_screen"), read_field("player_y_screen")
  local dvx, dvy
  if px and py then
    if _PREV_POS.x then
      -- NES coords wrap in 8 bits; keep the delta small and signed.
      local function d(a, b) local v = a - b; if v > 127 then v = v - 256 elseif v < -128 then v = v + 256 end; return v end
      dvx, dvy = d(px, _PREV_POS.x), d(py, _PREV_POS.y)
    end
    _PREV_POS.x, _PREV_POS.y = px, py
  end
  local L = CONFIG.layout.pos_panel
  draw_panel(L.x, L.y + 38, 60, {
    { text = "dVX " .. (dvx and dvx or "--") .. " (deriv)", color = dvx and CONFIG.colors.ok or CONFIG.colors.dim },
    { text = "dVY " .. (dvy and dvy or "--") .. " (deriv)", color = dvy and CONFIG.colors.ok or CONFIG.colors.dim },
    line("ground_air_flag"),
  })
  -- Derived velocity vector, world-anchored (only with a real position).
  if dvx and dvy and px and py then
    local s = CONFIG.layout.vector_scale
    gui.drawLine(px, py, px + dvx * s, py + dvy * s, CONFIG.colors.vec)
  end
  -- Ground marker from the confirmed flag: 2=grounded, 1=airborne.
  local gnd = read_field("ground_air_flag")
  if gnd ~= nil and px and py then
    gui.drawPixel(px, py + 2, gnd == 2 and CONFIG.colors.gnd_on or CONFIG.colors.gnd_off)
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
  -- Reflected row: the game's own $4016 latch. Its bit order is the GAME's
  -- (R=01 L=02 D=04 U=08 St=10 Se=20 B=40 A=80 — tcsurfdesign.md), NOT the
  -- RetroPad mask above; decode it independently rather than remapping.
  local refl = read_field("pad_latch_p1")
  if refl ~= nil then
    local GB = { {8,"U"},{4,"D"},{2,"L"},{1,"R"},{0x20,"s"},{0x10,"S"},{0x40,"B"},{0x80,"A"} }
    local g = {}
    for _, b in ipairs(GB) do
      g[#g + 1] = (math.floor(refl / b[1]) % 2 == 1 and refl >= b[1]) and b[2] or "."
    end
    txt = txt .. " ram " .. table.concat(g)
  end
  gui.text(2, CONFIG.layout.input_y, txt, CONFIG.colors.ok)
end

local function draw_mode()
  local L = CONFIG.layout.mode_panel
  local v = read_field("mode_index")
  local mode_txt, mode_col
  if v == nil then
    mode_txt, mode_col = "MODE --", CONFIG.colors.dim
  else
    mode_txt, mode_col = (CONFIG.mode_names[v] or ("MODE " .. v)), CONFIG.colors.ok
  end
  -- Timer from the three confirmed digit bytes (tens:ones.tenths of seconds).
  local t10, t1, tt = read_field("time_sec_tens"), read_field("time_sec_ones"), read_field("time_tenths")
  local time_txt, time_col = "TIME --", CONFIG.colors.dim
  if t10 and t1 and tt then
    time_txt = string.format("TIME %d%d.%d", t10, t1, tt)
    time_col = CONFIG.colors.ok
  end
  -- character (A/B pedestal) and true lives count
  local ch = read_field("char_select_latch")
  local ch_txt = ch == nil and "CHR --" or ("CHR " .. (ch == 0 and "A" or "B"))
  draw_panel(L.x, L.y, 58, {
    { text = mode_txt, color = mode_col },
    { text = time_txt, color = time_col },
    { text = ch_txt, color = ch and CONFIG.colors.ok or CONFIG.colors.dim },
    line("lives_count"),
  })
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
