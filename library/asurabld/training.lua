-- training.lua — Asura Blade behavioral extras for RustRetro (Lua API v3).
--
-- THE WORKED EXAMPLE of the v3 script contract (docs/game-profiles.md):
--
--   * LOGIC LIVES ONCE, IN RUST; LUA ASKS VIA BINDINGS. Native training mode
--     (F5 / the Training panel / `--training`, src/training.rs) owns ALL round
--     enforcement — credits top-up, timer hold, health refill — and the
--     engine owns the controllable gate. Earlier versions of this script
--     re-implemented that trio with its own CONFIG switches, which meant
--     turning native training OFF left the script still refilling health and
--     arcade mode was unplayable with the script loaded (and its private gate
--     copy had gone stale, missing the gate-v3 char-select term). All of that
--     is DELETED here, not guarded: this script asks `training.enabled()`,
--     `game.controllable()`, etc., and never writes game memory at all.
--
--   * ADDRESSES ARE RESOLVED BY NAME from the loaded game profile
--     (library/asurabld/asurabld.profile.json) via `game.addr()` /
--     `game.block1()` / `game.field_off()` — no raw addresses in script
--     source. library/asurabld/asurabld.md remains the literate evidence for
--     every value; the profile is its machine-readable extract.
--
-- What remains here is BEHAVIOR BEYOND THE ENGINE'S SCHEMA: a scripted port-1
-- dummy driver (stand/crouch/jump/block/replay) with an in-memory input
-- record/replay buffer, hitstun tracking from the combo counters, and a stat
-- overlay that demonstrates the v3 read bindings (gate, training, shadow,
-- recorder state).
--
-- SANDBOX NOTE: this VM only loads the `table`/`string`/`math` stdlibs (see
-- src/lua_engine.rs) — there is NO `io`, `os`, or `package`. The input
-- replay buffer below therefore CANNOT persist to disk; it is in-memory
-- only, and reloading the script (F10 "Reload", or a fresh `--script` load)
-- discards it because `LuaEngine::reload()` throws away the whole VM. To
-- record and then switch CONFIG.dummy to "replay" WITHOUT losing the buffer,
-- mutate the live CONFIG via the MCP `run_lua` tool (same VM, not a fresh
-- one): run_lua("CONFIG.record = false; CONFIG.dummy = 'replay'").

-- ══════════════════════════════ API GUARD ═══════════════════════════════════

if not _RUSTRETRO_API or _RUSTRETRO_API < 3 then
  error(
    "training.lua requires RustRetro Lua API v3+ (game.*, training.*, "
    .. "shadow.*, record.*, emu.paused). This build reports _RUSTRETRO_API="
    .. tostring(_RUSTRETRO_API)
    .. " — update RustRetro before loading this script."
  )
end

-- ══════════════════════════════ CONFIG ══════════════════════════════════════
-- Edit these, then reload via F10, or mutate the live `CONFIG` table with the
-- MCP `run_lua` tool, e.g. run_lua("CONFIG.dummy = 'crouch'"). Every field is
-- read fresh each frame, so a run_lua mutation takes effect on the very next
-- frame with no reload. (Enforcement switches live in the NATIVE Training
-- panel now, not here — see the header.)

CONFIG = {
  -- Scripted dummy for port 1 (extends the native dummy with "replay"):
  --   "off" | "stand" | "crouch" | "jump" | "block" | "replay"
  -- Only drives while native training is ON with its own dummy set to Free,
  -- and never while the shadow bot is on — see dummy_tick.
  dummy = "off",

  -- Set true to arm capture of port-0 (human) input into the in-memory
  -- replay buffer; flip back to false to stop and keep the buffer for
  -- CONFIG.dummy = "replay". (Unrelated to `record.*`, the native jsonl
  -- session recorder, whose state the overlay also shows.)
  record = false,

  overlay_enabled = true,
}

-- ══════════════════════════════ ADDRESSES (RESOLVED, NOT RESTATED) ═════════
-- Resolved ONCE at script load from the game profile. Names match
-- asurabld.profile.json's `fighter_fields` / `globals` tables.

local BLOCK1 = game.block1()
local BLOCK2 = game.block2()

local OFF = {
  x          = game.field_off("x"),        -- screen X (u16 be)
  y          = game.field_off("y"),        -- screen Y (u16 be)
  facing     = game.field_off("facing"),   -- 0 = facing left (u8)
  hp_actual  = game.field_off("health"),   -- real health byte (u8, max 0xEF)
  meter      = game.field_off("meter"),    -- super meter (u8)
  meter_max  = game.field_off("meter_max"),-- per-character max meter (u8)
}

-- Cross-block combo counters double as "opponent in hitstun" flags
-- (peon2/fbneo-training-mode semantics; see asurabld.md).
local COMBO_ON_B2 = game.addr("combo_on_b2") -- nonzero: BLOCK1 comboing BLOCK2
local COMBO_ON_B1 = game.addr("combo_on_b1") -- nonzero: BLOCK2 comboing BLOCK1

assert(BLOCK1 and BLOCK2 and OFF.x and OFF.hp_actual and COMBO_ON_B2,
  "training.lua: game profile is missing expected fields/globals")

-- ══════════════════════════════ DUMMY CONTROL ═══════════════════════════════
-- Port 1 is the dummy. "block" holds AWAY from the other fighter using live
-- block X positions: BLOCK2 is treated as the dummy's own fighter (matching
-- peon2's P1/P2 labeling of these same two blocks) and BLOCK1 as the
-- opponent. See asurabld.md's "CRITICAL caveat" — block-to-port assignment
-- isn't proven fixed across modes, so this is a documented best-effort
-- default, not a certainty.
--
-- OWNERSHIP RULE (do not weaken): `input.set(1, ...)` overwrites ALL of port
-- 1's injected hold counters, so a script driving port 1 every frame would
-- fight the NATIVE training dummy (training.dummy() ~= "free") and stomp the
-- SHADOW bot's port-1 injection. The driver therefore runs ONLY while native
-- training is enabled, the native dummy is left in "free", and the shadow is
-- not on (shadow.on() is nil with no model, false when toggled off — only
-- `== true` blocks). On losing any of those conditions it releases its held
-- input ONCE (a single input.set(1, {})) and then stays completely silent, so
-- whoever owns port 1 next starts from a clean slate and is never overwritten.

local function block_dummy_input()
  local self_x  = memory.read_u16_be(BLOCK2 + OFF.x)
  local other_x = memory.read_u16_be(BLOCK1 + OFF.x)
  if self_x <= other_x then
    return { left = true }   -- dummy is left of opponent -> back = left
  else
    return { right = true }  -- dummy is right of opponent -> back = right
  end
end

local warned_bad_dummy = nil
local dummy_was_driving = false

local function dummy_tick(controllable)
  local may_drive = training.enabled()
    and training.dummy() == "free"
    and shadow.on() ~= true
    and controllable
    and CONFIG.dummy ~= "off"

  if not may_drive then
    -- Release once on the transition out of driving; never touch port 1
    -- again after that (see the ownership rule above).
    if dummy_was_driving then
      pcall(input.set, 1, {})
      dummy_was_driving = false
    end
    return
  end
  dummy_was_driving = true

  if CONFIG.dummy == "stand" then
    input.set(1, {})
  elseif CONFIG.dummy == "crouch" then
    input.set(1, { down = true })
  elseif CONFIG.dummy == "jump" then
    input.set(1, { up = true })
  elseif CONFIG.dummy == "block" then
    input.set(1, block_dummy_input())
  elseif CONFIG.dummy == "replay" then
    input.set(1, replay_playback())
  else
    if warned_bad_dummy ~= CONFIG.dummy then
      warned_bad_dummy = CONFIG.dummy
      console.log("training.lua: unknown CONFIG.dummy '" .. tostring(CONFIG.dummy)
        .. "' (expected off/stand/crouch/jump/block/replay) — treating as off")
    end
  end
end

-- ══════════════════════════════ RECORD / REPLAY BUFFER ══════════════════════
-- In-memory only (see sandbox note at top). Captures input.get(0) — the
-- human/P1 port — each frame while CONFIG.record is true, up to RECORD_CAP
-- frames (~10s at 60fps). Flipping CONFIG.record back to false stops capture;
-- the buffer then feeds CONFIG.dummy="replay", looping onto port 1 with exact
-- original timing (one array slot = one frame).

local RECORD_CAP = 600

-- Global (not local) so the MCP `run_lua` tool can introspect it for
-- verification (e.g. `#recording.buffer`) — run_lua executes as a fresh
-- top-level chunk in the same VM, which can only see globals.
recording = { buffer = {}, playback_idx = 1 }
local record_was_armed = false

local function record_tick()
  if CONFIG.record and not record_was_armed then
    recording.buffer = {}
    recording.playback_idx = 1
    console.log(string.format(
      "training.lua: replay capture ARMED (port 0, cap %d frames, in-memory only)",
      RECORD_CAP))
  end

  if CONFIG.record then
    if #recording.buffer < RECORD_CAP then
      table.insert(recording.buffer, input.get(0))
    else
      CONFIG.record = false
      console.log(string.format(
        "training.lua: replay buffer full (%d frames) — stopped", RECORD_CAP))
    end
  elseif record_was_armed then
    console.log(string.format(
      "training.lua: replay capture stopped (%d frames)", #recording.buffer))
  end

  record_was_armed = CONFIG.record
end

local warned_empty_replay = false

function replay_playback()
  if #recording.buffer == 0 then
    if not warned_empty_replay then
      warned_empty_replay = true
      console.log("training.lua: dummy='replay' but no capture yet (set CONFIG.record=true first)")
    end
    return {}
  end
  warned_empty_replay = false
  local mask = recording.buffer[recording.playback_idx]
  recording.playback_idx = recording.playback_idx + 1
  if recording.playback_idx > #recording.buffer then
    recording.playback_idx = 1 -- loop
  end
  return mask
end

-- ══════════════════════════════ HITSTUN TRACKING ════════════════════════════
-- "Hitstun flag" = the relevant combo counter CHANGED within the last
-- HITSTUN_WINDOW frames, not merely nonzero — the counters linger after
-- combos (see CLAUDE.md gotchas). The window comes from the profile's
-- calibration table (the same constant the feature pipeline uses).

local HITSTUN_WINDOW = game.calibration("HITSTUN_RECENT_FRAMES") or 20

local combo_track = {
  b1_on_b2_prev = -1, b1_on_b2_change_frame = -9999, -- B2 in hitstun
  b2_on_b1_prev = -1, b2_on_b1_change_frame = -9999, -- B1 in hitstun
}

local function update_hitstun_tracking()
  local fc = emu.framecount()
  local c1 = memory.read_u8(COMBO_ON_B2)
  local c2 = memory.read_u8(COMBO_ON_B1)

  if c1 ~= combo_track.b1_on_b2_prev then
    combo_track.b1_on_b2_change_frame = fc
    combo_track.b1_on_b2_prev = c1
  end
  if c2 ~= combo_track.b2_on_b1_prev then
    combo_track.b2_on_b1_change_frame = fc
    combo_track.b2_on_b1_prev = c2
  end

  local b2_hitstun = (fc - combo_track.b1_on_b2_change_frame) < HITSTUN_WINDOW
  local b1_hitstun = (fc - combo_track.b2_on_b1_change_frame) < HITSTUN_WINDOW
  return b1_hitstun, b2_hitstun
end

-- ══════════════════════════════ OVERLAY ═════════════════════════════════════
-- Demonstrates the v3 read bindings: gate verdict, native training state,
-- shadow state, native recorder state — all ASKED, none re-derived.

local C_TITLE  = 0x60E0FFFF
local C_TEXT   = 0xFFFFFFFF
local C_DIM    = 0xB0B0B0FF
local C_GOOD   = 0x60FF80FF
local C_WARN   = 0xFF5050FF
local C_HIT    = 0xFFD060FF

local ROW_H = 7
local X0, Y0 = 2, 2

local function block_line(label, base)
  local hp   = memory.read_u8(base + OFF.hp_actual)
  local mtr  = memory.read_u8(base + OFF.meter)
  local mmax = memory.read_u8(base + OFF.meter_max)
  local x    = memory.read_u16_be(base + OFF.x)
  local y    = memory.read_u16_be(base + OFF.y)
  local face = memory.read_u8(base + OFF.facing)
  local facs = (face == 0) and "L" or "R"
  return string.format("%s HP:%3d/EF MTR:%3d/%-3d X:%3d Y:%3d F:%s",
    label, hp, mtr, mmax, x, y, facs)
end

local function draw_overlay(controllable, b1_hit, b2_hit)
  local y = Y0
  local function line(s, c)
    gui.text(X0, y, s, c or C_TEXT, 1)
    y = y + ROW_H
  end

  line("ASURABLD SCRIPT (API V3)", C_TITLE)
  line("CONTROLLABLE: " .. (controllable and "YES" or "no"),
    controllable and C_GOOD or C_DIM)

  -- Native training mode (the enforcement owner).
  local t_on = training.enabled()
  line(string.format("TRAINING:%s DUMMY:%s REFILL:%s",
    t_on and "ON" or "off", training.dummy(),
    training.refill() and "ON" or "off"),
    t_on and C_GOOD or C_DIM)

  -- Shadow bot: nil = no model loaded, false = loaded but off, true = on.
  local s_on = shadow.on()
  if s_on == nil then
    line("SHADOW: NO MODEL", C_DIM)
  else
    line(string.format("SHADOW:%s %s", s_on and "ON" or "off",
      shadow.model() or "?"), s_on and C_GOOD or C_DIM)
  end

  -- Native jsonl session recorder.
  if record.active() then
    line(string.format("REC %d FRAMES %s", record.frames(),
      record.path() or "?"), C_WARN)
  else
    line("REC: off", C_DIM)
  end

  line(block_line("B1", BLOCK1), C_TEXT)
  line(block_line("B2", BLOCK2), C_TEXT)

  line(string.format("HITSTUN  B1:%s  B2:%s",
    b1_hit and "YES" or "no", b2_hit and "YES" or "no"),
    (b1_hit or b2_hit) and C_HIT or C_DIM)

  -- Scripted dummy + replay buffer state.
  local buf_state
  if CONFIG.record then
    buf_state = string.format("CAPTURING %d/%d", #recording.buffer, RECORD_CAP)
  elseif #recording.buffer > 0 then
    buf_state = string.format("buffer %d frames", #recording.buffer)
  else
    buf_state = "buffer empty"
  end
  line(string.format("SCRIPT DUMMY:%s  %s", CONFIG.dummy, buf_state), C_DIM)
end

-- ══════════════════════════════ ONE-SHOT HELPERS ════════════════════════════
-- Globals, callable from the F10 script panel's inline-Lua box or the MCP
-- `run_lua` tool (same VM as this loaded script). Position resets and
-- finish-round are NATIVE now (F2/F4 / the Training panel) — only the
-- savestate wrappers remain script-side.

function arena_save(slot)
  local ok, err = pcall(savestate.save, slot)
  console.log(string.format("arena_save(%s): %s", tostring(slot),
    ok and "queued" or ("ERROR: " .. tostring(err))))
  return ok
end

function arena_load(slot)
  local ok, err = pcall(savestate.load, slot)
  console.log(string.format("arena_load(%s): %s", tostring(slot),
    ok and "queued" or ("ERROR: " .. tostring(err))))
  return ok
end

-- ══════════════════════════════ MAIN LOOP ═══════════════════════════════════

event.onframeend(function()
  local controllable = game.controllable()

  record_tick()
  dummy_tick(controllable)
  local b1_hit, b2_hit = update_hitstun_tracking()
  if CONFIG.overlay_enabled then
    draw_overlay(controllable, b1_hit, b2_hit)
  end
end)

console.log(string.format(
  "training.lua (API v3) loaded: B1=0x%06X B2=0x%06X — script dummy=%s "
  .. "(enforcement is native training mode's job)",
  BLOCK1, BLOCK2, CONFIG.dummy))
