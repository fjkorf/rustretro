-- frame_meter.lua — a fighting-game FRAME METER overlay for RustRetro.
--
-- A frame meter is the training-mode tool that shows, for each player, a
-- scrolling timeline of their per-frame ACTION STATE — startup / active /
-- recovery / hitstun / blockstun / neutral — as a row of colored cells, with
-- the current run length (in frames) printed at the live edge. It's how you read
-- move frame data ("7f startup, 3f active, 21f recovery") at a glance.
--
-- This script is a GENERIC, REUSABLE ENGINE: the rendering, scrolling history,
-- run-length counter, and legend are game-agnostic. The ONLY game-specific parts
-- are in the CONFIG block — the RAM address that holds each player's action
-- state, and the `categorize` function that maps that raw value to a category.
-- Everything below CONFIG is the engine; you shouldn't need to touch it.
--
-- ── How to adapt it to YOUR game ──────────────────────────────────────────────
-- The hard part of any frame meter is finding the action-state byte. Use the
-- RustRetro RE tools to discover it, then drop the address + mapping into CONFIG:
--   1. Pause in neutral; note candidate RAM via the RAM search / `scan_regions`
--      (look for a small RAM byte that flips the instant a move starts).
--   2. Do a move; `ram_search` for a value that changed, narrow over several
--      reps until one byte tracks the move state.
--   3. Watch it (add_watch) while doing known moves to learn which values mean
--      startup vs active vs recovery, then encode that in `categorize`.
-- Until you've done that, the DEFAULT config below just buckets a raw RAM byte by
-- value so the meter VISIBLY scrolls on any running game — proving the pipeline,
-- exactly like hitbox_demo.lua. Replace it with real state for true frame data.
--
-- API used (v1): memory.read_u8, gui.drawBox, gui.drawText(x,y,s,color,scale),
-- event.onframeend, emu.framecount, console.log. Coordinates are GAME-PIXEL space.

-- ══════════════════════════════ CONFIG ═══════════════════════════════════════

-- Category keys → cell color (0xRRGGBBAA). Add/rename freely; `categorize` must
-- return one of these keys (or "neutral" as the safe default).
local COLORS = {
  neutral  = 0x303030C0, -- dark grey: idle / standing / walking
  startup  = 0x2A6CF0E0, -- blue:   move started, not yet hitting
  active   = 0xE02020E0, -- red:    hitbox is out (the dangerous frames)
  recovery = 0xE0A020E0, -- amber:  move is ending, vulnerable
  hitstun  = 0x20E040E0, -- green:  getting hit / in hitstun
  blockstun= 0x9020E0E0, -- purple: blocking
}
-- Draw order / legend order of the categories.
local CATEGORY_ORDER = { "neutral", "startup", "active", "recovery", "hitstun", "blockstun" }

-- Per-player config. `addr` is the guest address of the action-state byte.
-- NOTE: the defaults are PLACEHOLDERS chosen so the meter moves on a running
-- game (NES work RAM 0x0000-0x07FF; Genesis is 0xFF0000+). Replace with the real
-- per-character state addresses you discover.
local PLAYERS = {
  { label = "P1", addr = 0x0040 },
  { label = "P2", addr = 0x0060 },
}

-- Map a raw state byte to a category key. THIS is the game-specific brain.
-- The default is a generic bucketing of the byte so something shows; swap it for
-- your game's real state table (e.g. `if v==0 then return "neutral" elseif ...`).
local function categorize(v)
  if v == 0            then return "neutral"  end
  if v < 0x20          then return "startup"  end
  if v < 0x40          then return "active"   end
  if v < 0x80          then return "recovery" end
  if v < 0xC0          then return "hitstun"  end
  return "blockstun"
end

-- Layout (game-pixel space). HISTORY = how many past frames the meter shows.
local LAYOUT = {
  x = 8, y = 8,          -- top-left of the meter
  history = 64,          -- frames of timeline shown
  cell_w = 2,            -- px per frame cell
  row_h = 7,             -- px per player row (cell height = row_h - 1)
  label_w = 16,          -- px reserved on the left for the player label
  count_scale = 1,       -- text scale for the run-length number
  legend = true,         -- draw the category legend below the rows
}

-- ══════════════════════════════ ENGINE ══════════════════════════════════════
-- (game-agnostic; no need to edit below here)

-- Per-player ring of category keys, newest last. Pre-fill with neutral so the
-- meter is full-width from frame 1.
local history = {}
for i = 1, #PLAYERS do
  history[i] = {}
  for _ = 1, LAYOUT.history do history[i][#history[i] + 1] = "neutral" end
end

local last_fc = -1

-- Length of the current (newest-end) run of identical categories in `ring`.
local function current_run(ring)
  local n = #ring
  if n == 0 then return 0 end
  local last = ring[n]
  local run = 1
  for i = n - 1, 1, -1 do
    if ring[i] == last then run = run + 1 else break end
  end
  return run
end

local function sample_and_scroll()
  for i, p in ipairs(PLAYERS) do
    local cat = categorize(memory.read_u8(p.addr)) or "neutral"
    if COLORS[cat] == nil then cat = "neutral" end
    local ring = history[i]
    ring[#ring + 1] = cat
    -- Drop the oldest so the window stays fixed-width (scrolls left).
    while #ring > LAYOUT.history do table.remove(ring, 1) end
  end
end

local function draw_meter()
  local L = LAYOUT
  local x0 = L.x + L.label_w
  for i, p in ipairs(PLAYERS) do
    local row_y = L.y + (i - 1) * L.row_h
    local cell_h = L.row_h - 1

    -- Player label.
    gui.drawText(L.x, row_y, p.label, 0xFFFFFFFF, 1)

    -- The timeline of colored cells (oldest left → newest right).
    local ring = history[i]
    for j = 1, #ring do
      local cx = x0 + (j - 1) * L.cell_w
      local color = COLORS[ring[j]] or COLORS.neutral
      gui.drawBox(cx, row_y, cx + L.cell_w - 1, row_y + cell_h, color, 0x00000000)
    end

    -- Run-length of the live (rightmost) state, printed just past the meter.
    local run = current_run(ring)
    local label = ring[#ring] or "neutral"
    local tx = x0 + L.history * L.cell_w + 3
    gui.drawText(tx, row_y, string.format("%s %d", string.upper(label), run),
                 COLORS[label] and 0xFFFFFFFF or 0xFFFFFFFF, L.count_scale)
  end

  -- Legend: a swatch + name per category, in a row under the meter.
  if L.legend then
    local ly = L.y + #PLAYERS * L.row_h + 2
    local lx = L.x
    for _, key in ipairs(CATEGORY_ORDER) do
      gui.drawBox(lx, ly, lx + 4, ly + 4, COLORS[key], 0xFFFFFF40)
      gui.drawText(lx + 6, ly, string.upper(key), 0xFFFFFFFF, 1)
      lx = lx + 6 + (#key + 1) * 4  -- swatch + text width (cell width 4px/char)
    end
  end
end

event.onframeend(function()
  -- Only advance the timeline on a genuinely NEW emulated frame, so the meter
  -- freezes (rather than spamming duplicates) while the emulator is paused.
  local fc = emu.framecount()
  if fc ~= last_fc then
    last_fc = fc
    sample_and_scroll()
  end
  draw_meter()
end)

console.log(string.format("frame_meter.lua loaded (%d players, %d-frame window)",
            #PLAYERS, LAYOUT.history))
