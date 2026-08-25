-- training.lua — Asura Blade training-mode script for RustRetro (Lua API v2).
--
-- Standalone `--script` training surface: round-enforcement (timer/credits/
-- health), a port-1 "dummy" driver (stand/crouch/jump/block/replay), an
-- in-memory input recorder, a compact stat overlay, and one-shot helpers
-- (reset_positions/finish_round/arena_save/arena_load) callable from the F10
-- script panel or the MCP `run_lua` tool.
--
-- WHY THIS EXISTS: the native `--training` flag already does timer/credit/
-- health enforcement in Rust, but a script must be able to do the same job
-- standalone (someone launches with `--script` but forgets `--training`), and
-- must not fight the native path when both are armed (this script always
-- writes the same fixed target values, so re-asserting them is a no-op).
--
-- MEMORY MODEL: all addresses below are guest (68k) bus addresses inside the
-- Work RAM window (0x400000-0x40FFFF) declared in asurabld.busmap.json. See
-- library/asurabld/asurabld.md for how every address was verified. Ideas
-- (dummy control, record/replay, hitstun-from-combo-counter) are ported from
-- https://github.com/peon2/fbneo-training-mode's games/asurabld/asurabld.lua
-- and its code/replay.lua and code/input.lua — the ADDRESSES matched that
-- project's own asurabld.lua almost field-for-field, which cross-confirms
-- them independently of our own live RE session.
--
-- SANDBOX NOTE: this VM only loads the `table`/`string`/`math` stdlibs (see
-- src/lua_engine.rs) — there is NO `io`, `os`, or `package`. That means the
-- input recorder below CANNOT persist to disk; it is in-memory only, and
-- reloading the script (F10 "Reload", or a fresh `--script` load) discards it
-- because `LuaEngine::reload()` throws away the whole VM and re-runs the file
-- from scratch. If you want to record, then switch CONFIG.dummy to "replay"
-- WITHOUT losing the buffer, do it live via the MCP `run_lua` tool (it runs
-- in the SAME VM as this script, not a fresh one) — e.g.
-- run_lua("CONFIG.record = false; CONFIG.dummy = 'replay'") — rather than
-- editing this file and reloading.

-- ══════════════════════════════ API GUARD ═══════════════════════════════════

if not _RUSTRETRO_API or _RUSTRETRO_API < 2 then
  error(
    "training.lua requires RustRetro Lua API v2+ (memory.write*, savestate.*, "
    .. "input.*, gui.text). This build reports _RUSTRETRO_API="
    .. tostring(_RUSTRETRO_API)
    .. " — update RustRetro before loading this script."
  )
end

-- ══════════════════════════════ CONFIG ══════════════════════════════════════
-- Edit these, then either reload via F10 (safe for everything except an
-- in-flight recording buffer — see the sandbox note above) or mutate the
-- live `CONFIG` table with the MCP `run_lua` tool, e.g.:
--   run_lua("CONFIG.dummy = 'crouch'")
-- Every field below is read fresh each frame, so a run_lua mutation takes
-- effect on the very next frame with no reload at all.

CONFIG = {
  -- 1) Enforcement — active only while enforce_round_live() below is true.
  credits_enabled       = true,   -- top up credits once/second
  credits_target        = 9,
  timer_hold_enabled     = true,  -- pin the round timer so it never expires
  timer_hold_sec        = 0x85,   -- BCD seconds byte written to $40000A
  timer_hold_sub        = 0x03,   -- subsecond byte written to $40000B
  health_refill_enabled = true,   -- keep both fighters out of KO range
  refill_below          = 0x40,   -- refill when EITHER health byte drops below this
  refill_value          = 0xEF,   -- value written to both health bytes on refill

  -- 2) Dummy control (port 1 = the CPU/2P slot driven by input.set(1, ...)).
  --    "off" | "stand" | "crouch" | "jump" | "block" | "replay"
  dummy = "off",

  -- 3) Record/replay. Set record=true to arm capture of port-0 (human) input;
  --    flipping it back to false stops the recording and leaves the buffer in
  --    place for "replay" mode. See the sandbox note above about persistence.
  record = false,

  -- 4) Overlay.
  overlay_enabled = true,
}

-- ══════════════════════════════ ADDRESSES ═══════════════════════════════════
-- Source: library/asurabld/asurabld.md ("Fighter data blocks — the corrected
-- model", "round-timer", "health-blocks", "system-control"). All confirmed
-- live 2026-08-24 and cross-checked against peon2/fbneo-training-mode's own
-- asurabld.lua (same offsets, same stride).

local BLOCK1        = 0x403798   -- fighter block 1 (peon2 labels this P1)
local BLOCK2        = 0x40454C   -- fighter block 2 (peon2 labels this P2); stride 0xDB4
local BLOCK_STRIDE  = 0x0DB4
assert(BLOCK2 - BLOCK1 == BLOCK_STRIDE, "block stride mismatch — re-verify addresses")

local OFF = {
  x          = 0x54,   -- screen X (u16 be)
  y          = 0x56,   -- screen Y (u16 be)
  facing     = 0x61,   -- 0 = facing left (u8)
  hp_actual  = 0x177,  -- real health byte (u8, max 0xEF)
  hp_display = 0x179,  -- displayed-bar value, chases hp_actual downward (u8)
  meter      = 0x17B,  -- super meter (u8)
  meter_max  = 0x17F,  -- per-character max meter constant (u8)
  char_id    = 0x639,  -- character id (u8)
}

local ADDR = {
  credits          = 0x40655D,
  timer_sec        = 0x40000A,  -- BCD seconds
  timer_sub        = 0x40000B,
  finish_round_now = 0x400000,  -- write 0 = finish round now
  hop_round_over   = 0x40646E,
  hop_abort        = 0x403678,
  hop_match_end    = 0x402A32,
  -- combo counters double as "opponent in hitstun" flags (peon2 semantics):
  combo_b1_on_b2   = 0x4041E7,  -- nonzero: BLOCK1 is comboing BLOCK2 (B2 in hitstun)
  combo_b2_on_b1   = 0x40470B,  -- nonzero: BLOCK2 is comboing BLOCK1 (B1 in hitstun)
}

-- Round-start reference positions (task spec / live-verified).
local START_X_LEFT  = 84
local START_X_RIGHT = 232
local START_Y       = 216

-- ══════════════════════════════ WRITE GATE HELPERS ══════════════════════════
-- memory.writebyte/writeword raise a Lua error naming the `lua_writes_enabled`
-- gate when writes are locked (no --training, no MCP enable_writes). We pcall
-- every write so the script degrades to "overlay warning" instead of an error
-- spam, and track the gate state for the HUD.

local writes_locked = false

local function note_write_result(ok)
  local locked_now = not ok
  if locked_now ~= writes_locked then
    writes_locked = locked_now
    if writes_locked then
      console.log(
        "training.lua: memory writes BLOCKED — writes locked: launch with "
        .. "--training or use the MCP enable_writes tool to arm the gate"
      )
    else
      console.log("training.lua: memory writes armed (gate open)")
    end
  end
end

local function try_write_u8(addr, v)
  local ok = pcall(memory.writebyte, addr, v)
  note_write_result(ok)
  return ok
end

local function try_write_u16(addr, v)
  local ok = pcall(memory.writeword, addr, v)
  note_write_result(ok)
  return ok
end

-- Non-destructive gate probe: read the credits byte back and write the exact
-- same value. This exercises memory.writebyte (and therefore keeps
-- `writes_locked` accurate) even when every enforcement toggle is off or no
-- round is live, without changing any game state.
local last_probe_frame = -9999
local function probe_write_gate()
  local fc = emu.framecount()
  if fc - last_probe_frame < 30 then return end
  last_probe_frame = fc
  local cur = memory.read_u8(ADDR.credits)
  try_write_u8(ADDR.credits, cur)
end

-- ══════════════════════════════ ROUND STATE ═════════════════════════════════

local function bcd_digits_valid(b)
  return (b % 16) <= 9 and (math.floor(b / 16) % 16) <= 9
end

-- "A live round": both hop-flag/abort/match-end latches are clear, both
-- fighters' actual health is inside 1..0xEF (0 = KO'd / no fight loaded), and
-- the timer byte is valid BCD. Mirrors the definition in the task spec and
-- library/asurabld/asurabld.md's hop-flag table.
local function round_live()
  if memory.read_u8(ADDR.hop_round_over) ~= 0 then return false end
  if memory.read_u8(ADDR.hop_abort) ~= 0 then return false end
  if memory.read_u8(ADDR.hop_match_end) ~= 0 then return false end

  local h1 = memory.read_u8(BLOCK1 + OFF.hp_actual)
  local h2 = memory.read_u8(BLOCK2 + OFF.hp_actual)
  if h1 < 1 or h1 > 0xEF or h2 < 1 or h2 > 0xEF then return false end

  local t = memory.read_u8(ADDR.timer_sec)
  if not bcd_digits_valid(t) then return false end

  return true
end

-- ══════════════════════════════ 1) ENFORCEMENT ══════════════════════════════

local last_credits_write_frame = -9999

local function enforce_tick()
  if CONFIG.credits_enabled then
    local fc = emu.framecount()
    if fc - last_credits_write_frame >= 60 then -- ~once/second at 60fps
      last_credits_write_frame = fc
      local cur = memory.read_u8(ADDR.credits)
      if cur ~= CONFIG.credits_target then
        try_write_u8(ADDR.credits, CONFIG.credits_target)
      end
    end
  end

  if CONFIG.timer_hold_enabled then
    local sec = memory.read_u8(ADDR.timer_sec)
    local sub = memory.read_u8(ADDR.timer_sub)
    if sec ~= CONFIG.timer_hold_sec then
      try_write_u8(ADDR.timer_sec, CONFIG.timer_hold_sec)
    end
    if sub ~= CONFIG.timer_hold_sub then
      try_write_u8(ADDR.timer_sub, CONFIG.timer_hold_sub)
    end
  end

  if CONFIG.health_refill_enabled then
    for _, base in ipairs({ BLOCK1, BLOCK2 }) do
      local a = memory.read_u8(base + OFF.hp_actual)
      local b = memory.read_u8(base + OFF.hp_display)
      if a < CONFIG.refill_below or b < CONFIG.refill_below then
        try_write_u8(base + OFF.hp_actual, CONFIG.refill_value)
        try_write_u8(base + OFF.hp_display, CONFIG.refill_value)
      end
    end
  end
end

-- ══════════════════════════════ 2) DUMMY CONTROL ════════════════════════════
-- Port 1 is the dummy. "block" holds AWAY from the other fighter using live
-- block X positions: BLOCK2 is treated as the dummy's own fighter (matching
-- peon2's P1/P2 labeling of these same two blocks) and BLOCK1 as the
-- opponent. See asurabld.md's "CRITICAL caveat" — block-to-port assignment
-- isn't proven fixed across modes, so this is a documented best-effort
-- default, not a certainty.

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

local function dummy_tick(live)
  if not live then
    -- Outside a live round, release the dummy's held inputs so it doesn't
    -- wander through menus/attract while a mode is configured.
    if CONFIG.dummy ~= "off" then
      pcall(input.set, 1, {})
    end
    return
  end

  if CONFIG.dummy == "off" then
    -- do nothing; input.set's 2-frame hold means simply not calling it
    -- releases whatever was previously held.
  elseif CONFIG.dummy == "stand" then
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

-- ══════════════════════════════ 3) RECORD / REPLAY ══════════════════════════
-- In-memory only (see sandbox note at top). Captures input.get(0) — the
-- human/P1 port — each frame while CONFIG.record is true, up to RECORD_CAP
-- frames (~10s at 60fps). Flipping CONFIG.record back to false (by any
-- means — reload or a live run_lua mutation) stops capture; the buffer then
-- feeds CONFIG.dummy="replay", looping onto port 1 with 1-frame-per-frame
-- timing (i.e. exact original timing, since one array slot = one frame).

local RECORD_CAP = 600

-- Global (not local) so the MCP `run_lua` tool can introspect it for
-- verification (e.g. `#recording.buffer`) — run_lua executes as a fresh
-- top-level chunk in the same VM, which can only see globals, not another
-- chunk's locals.
recording = { buffer = {}, playback_idx = 1 }
local record_was_armed = false

-- Optional file persistence: the sandbox is TABLE|STRING|MATH only (see
-- src/lua_engine.rs), so `io` does not exist as a global. This checks
-- defensively (in case a future build widens the sandbox) but today always
-- takes the in-memory-only path and logs that finding once.
local io_available = (io ~= nil and type(io.open) == "function")
local io_note_logged = false

local function record_tick()
  if CONFIG.record and not record_was_armed then
    recording.buffer = {}
    recording.playback_idx = 1
    console.log(string.format(
      "training.lua: recording ARMED (capturing port 0, cap %d frames)", RECORD_CAP))
    if not io_note_logged then
      io_note_logged = true
      if io_available then
        console.log("training.lua: io available — recording could persist to disk (not implemented; in-memory buffer only)")
      else
        console.log("training.lua: io sandbox CONFIRMED BLOCKED (table/string/math only) — recording is in-memory only and is lost on script reload")
      end
    end
  end

  if CONFIG.record then
    if #recording.buffer < RECORD_CAP then
      table.insert(recording.buffer, input.get(0))
    else
      CONFIG.record = false
      console.log(string.format(
        "training.lua: recording buffer full (%d frames) — stopped", RECORD_CAP))
    end
  elseif record_was_armed then
    console.log(string.format(
      "training.lua: recording stopped (%d frames captured)", #recording.buffer))
  end

  record_was_armed = CONFIG.record
end

local warned_empty_replay = false

function replay_playback()
  if #recording.buffer == 0 then
    if not warned_empty_replay then
      warned_empty_replay = true
      console.log("training.lua: dummy='replay' but no recording captured yet (set CONFIG.record=true first)")
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
-- "Hitstun flag" = the relevant combo counter CHANGED within the last 20
-- frames (per spec), not merely nonzero — a still-nonzero-but-unchanging
-- counter between hits shouldn't keep flashing hitstun.

local HITSTUN_WINDOW = 20

local combo_track = {
  b1_on_b2_prev = -1, b1_on_b2_change_frame = -9999, -- B2 in hitstun
  b2_on_b1_prev = -1, b2_on_b1_change_frame = -9999, -- B1 in hitstun
}

local function update_hitstun_tracking()
  local fc = emu.framecount()
  local c1 = memory.read_u8(ADDR.combo_b1_on_b2)
  local c2 = memory.read_u8(ADDR.combo_b2_on_b1)

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

-- ══════════════════════════════ 4) OVERLAY ══════════════════════════════════

local C_TITLE  = 0x60E0FFFF
local C_TEXT   = 0xFFFFFFFF
local C_DIM    = 0xB0B0B0FF
local C_GOOD   = 0x60FF80FF
local C_WARN   = 0xFF5050FF
local C_HIT    = 0xFFD060FF

local ROW_H = 7
local X0, Y0 = 2, 2

local function block_line(label, base, color)
  local hp   = memory.read_u8(base + OFF.hp_actual)
  local mtr  = memory.read_u8(base + OFF.meter)
  local mmax = memory.read_u8(base + OFF.meter_max)
  local x    = memory.read_u16_be(base + OFF.x)
  local y    = memory.read_u16_be(base + OFF.y)
  local face = memory.read_u8(base + OFF.facing)
  local facs = (face == 0) and "L" or "R"
  return string.format("%s HP:%3d/EF MTR:%3d/%-3d X:%3d Y:%3d F:%s",
    label, hp, mtr, mmax, x, y, facs), color
end

local function draw_overlay(live, b1_hit, b2_hit)
  local y = Y0
  local function line(s, c)
    gui.text(X0, y, s, c or C_TEXT, 1)
    y = y + ROW_H
  end

  line("ASURABLD TRAINING", C_TITLE)
  line("ROUND: " .. (live and "LIVE" or "--"), live and C_GOOD or C_DIM)

  if writes_locked then
    line("WRITES LOCKED: --training or MCP enable_writes", C_WARN)
  end

  local s1 = block_line("B1", BLOCK1)
  line(s1, C_TEXT)
  local s2 = block_line("B2", BLOCK2)
  line(s2, C_TEXT)

  line(string.format("HITSTUN  B1:%s  B2:%s",
    b1_hit and "YES" or "no", b2_hit and "YES" or "no"),
    (b1_hit or b2_hit) and C_HIT or C_DIM)

  local rec_state
  if CONFIG.record then
    rec_state = string.format("REC %d/%d", #recording.buffer, RECORD_CAP)
  elseif #recording.buffer > 0 then
    rec_state = string.format("stopped (%d frames)", #recording.buffer)
  else
    rec_state = "empty"
  end
  line(string.format("DUMMY:%s  %s", CONFIG.dummy, rec_state), C_DIM)
end

-- ══════════════════════════════ ONE-SHOT HELPERS ════════════════════════════
-- Globals, callable from the F10 script panel's inline-Lua box or the MCP
-- `run_lua` tool (same VM as this loaded script).

function reset_positions()
  local ok = true
  ok = try_write_u16(BLOCK1 + OFF.x, START_X_LEFT) and ok
  ok = try_write_u16(BLOCK1 + OFF.y, START_Y) and ok
  ok = try_write_u16(BLOCK2 + OFF.x, START_X_RIGHT) and ok
  ok = try_write_u16(BLOCK2 + OFF.y, START_Y) and ok
  console.log(string.format(
    "reset_positions(): B1->(%d,%d) B2->(%d,%d) %s",
    START_X_LEFT, START_Y, START_X_RIGHT, START_Y,
    ok and "OK" or "BLOCKED (writes locked)"))
  return ok
end

function finish_round()
  local ok = try_write_u8(ADDR.finish_round_now, 0)
  console.log(string.format("finish_round(): wrote 0 to 0x%06X %s",
    ADDR.finish_round_now, ok and "OK" or "BLOCKED (writes locked)"))
  return ok
end

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
  local live = round_live()

  probe_write_gate()
  if live then enforce_tick() end
  record_tick()
  dummy_tick(live)
  local b1_hit, b2_hit = update_hitstun_tracking()
  if CONFIG.overlay_enabled then
    draw_overlay(live, b1_hit, b2_hit)
  end
end)

console.log(string.format(
  "training.lua loaded: B1=0x%06X B2=0x%06X (stride 0x%X) — dummy=%s record=%s",
  BLOCK1, BLOCK2, BLOCK_STRIDE, CONFIG.dummy, tostring(CONFIG.record)))
