-- Dummy-state HUD — FAMILY-AGNOSTIC. Draws the BlockPunish phase on the game
-- screen so a silent dummy explains itself without opening the debugger:
--   ARMED       ready — the next trigger punishes. On button-block games
--               (MK2) that trigger is blocked CONTACT; on back-to-block
--               games (Asura Blade) it is the opponent COMMITTING an attack
--               in range, and the phase then reads "punishing (commit)".
--   guarding    the reactive guard window is open (back-to-block only —
--               these dummies stand neutral otherwise so they never retreat)
--   cooling Nf  waiting for the signal to go quiet before re-arming
--   punishing:  the selected option is executing
--
-- The phase string is computed ONCE in src/training.rs; this only draws it
-- (the panel reads the same value — no second state machine here).
--
-- Works for ANY game: it reads only the generic training.* bindings, never a
-- game address. Load with --script library/dummy_hud.lua, or via the F10 panel.

local COLOR_ARMED    = 0x4CAF50FF   -- green: ready
local COLOR_COOLING  = 0x9E9E9EFF   -- grey: waiting
local COLOR_PUNISH   = 0xFFC107FF   -- amber: acting
local X, Y = 4, 4

event.onframeend(function()
  if not training.enabled() then return end
  if training.dummy() ~= "block_punish" then return end
  local phase = training.punish_state()
  if phase == "" then return end

  local color = COLOR_COOLING
  if string.find(phase, "punishing", 1, true) then
    color = COLOR_PUNISH
  elseif string.find(phase, "ARMED", 1, true) then
    color = COLOR_ARMED
  end
  gui.text(X, Y, "DUMMY " .. phase, color)
end)
