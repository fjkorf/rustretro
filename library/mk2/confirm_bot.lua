-- confirm_bot.lua — a HIT-CONFIRM ATTACKER for MK2 arcade (Mileena, P1/port 0).
-- RustRetro Lua API v3.  Family: mk2, port: arcade.
--
-- ═══════════════════════════════════════════════════════════════════════════
-- WHAT THIS IS
-- ═══════════════════════════════════════════════════════════════════════════
-- The LIVE counterpart to shadow/demos/mk2/*.beats.json.  Those demos are
-- CHOREOGRAPHED: a recorded input slot replayed frame-exactly, so every run is
-- identical.  This is the same frame data turned into a REACTIVE policy, so
-- turn-taking EMERGES and no two runs are the same.
--
-- The one idea: MK2's far HK is a turn-decision that CANNOT be un-committed
-- (+3 on hit, −20 on block, `mileena/HK/far` rows 29–34 of
-- library/mk2/arcade.frames.json).  So the bot never throws it on a hunch.  It
-- throws a SAFE starter first (far HP: +13 on block, +4 on hit — rows 21/22),
-- reads the opponent's struct-health DELTA AMOUNT on the contact frame, and
-- only commits when that amount says HIT.  Blocked → it keeps its (+13) turn:
-- a chain jab inside the measured f12–f14 window, then it SPENDS the plus on
-- walking back into range rather than on guarding (guarding costs an 8–18
-- frame block-stance latch to get out of again).  Whiffed → it re-blocks and
-- re-spaces.  After CONFIG.max_string_actions it hands the turn back.
--
-- Everything the bot decides is read from live memory through profile
-- bindings (`game.read_field`), never guessed from animation, and every
-- constant in CONFIG cites the measured row or mk2.md section it comes from.
--
-- ═══════════════════════════════════════════════════════════════════════════
-- HOW TO RUN IT
-- ═══════════════════════════════════════════════════════════════════════════
-- (a) At launch, headless (agent / validation):
--
--   ./target/release-dev/rustretro \
--     --core ../FBNeo/src/burner/libretro/fbneo_libretro.dylib \
--     --rom ~/games/roms/mk2.zip --game library/mk2 \
--     --headless --mcp-port 4026 --pace 0 \
--     --training --script library/mk2/confirm_bot.lua
--
--   `--training` arms the Lua write gate and the native dummy; `--script`
--   loads this file BEFORE the first frame, so the bot is live from frame 1.
--   Then load an arena and configure the dummy over MCP:
--       load_state  path=shadow/arenas/mk2/m-gap-39.state
--       run_lua     script="BOT.setup('block_punish','all','fast')"
--
-- (b) At launch, windowed (a human plays P2):
--
--   ./target/release-dev/rustretro --core ... --rom ~/games/roms/mk2.zip \
--     --game library/mk2 --training --script library/mk2/confirm_bot.lua --scale 3
--
--   Then: F10 → the script panel shows this file loaded.  Press F5 for
--   training, and in the 🎯 Training panel set the dummy to **Free** —
--   `BOT.setup('free')` does the same over MCP.  YOU are P2.  The bot holds
--   port 0 only and never writes P2's buttons, so a second pad (or the
--   keyboard's P2 binds) fights it directly.  Turn the native dummy back on
--   (`block_punish`) to watch bot-vs-dummy instead.
--
-- (c) Into an ALREADY RUNNING session (the file is not on the command line):
--   Paste the whole file through MCP `run_lua`, or — much easier — use the
--   F10 script panel's file picker.  NOTE (CLAUDE.md): `LuaEngine::reload()`
--   destroys VM state, so a reload discards the trace buffer.  To change the
--   bot's behaviour WITHOUT losing the trace, mutate the live table instead:
--       run_lua  script="CONFIG.commit_enabled = false"
--   Every CONFIG field is re-read every frame, so that lands on the next one.
--
-- ═══════════════════════════════════════════════════════════════════════════
-- WHAT IT DOES NOT DO
-- ═══════════════════════════════════════════════════════════════════════════
--  * It never writes game memory.  Round enforcement (health refill, timer
--    hold, credits) belongs to native training mode (src/training.rs), per the
--    v3 script contract in docs/game-profiles.md.
--  * It never drives port 1.  The BlockPunish dummy rewrites port 1 every
--    frame; the bot holds port 0, which the dummy never stomps.
--  * It carries no raw addresses.  `game.read_field(block, name)` resolves
--    MK2's pointer-indirected `x` through the profile; a nil read means
--    ABSENT (a stale object-pool pointer this frame), NEVER 0 — the bot
--    declines to make a position decision on those frames instead of
--    inventing a gap.
--  * No wall clock.  Every duration below is in emulated frames.

-- ══════════════════════════════ API GUARD ═══════════════════════════════════

if not _RUSTRETRO_API or _RUSTRETRO_API < 3 then
  error("confirm_bot.lua requires RustRetro Lua API v3+ (game.read_field, "
     .. "training.set_reversal); this build reports _RUSTRETRO_API="
     .. tostring(_RUSTRETRO_API))
end

-- ══════════════════════════════ CONFIG ══════════════════════════════════════
-- Mutable live via MCP `run_lua` (e.g. run_lua("CONFIG.commit_enabled=false")).
-- EVERY number here cites the measurement it comes from.  A number with no
-- citation is a bug — see CLAUDE.md, "the frame lab".
--   [row N]   = library/mk2/arcade.frames.json, `moves[].id`
--   [mk2.md §] = library/mk2/mk2.md section title

CONFIG = {
  enabled = true,

  -- ── spacing bands (px, gap = |x1 − x2| via the object_ptr-resolved `x`) ──
  -- The SAME button is a different MOVE across Mileena's proximity boundary,
  -- and the chip AMOUNT says which one came out (24/4 = 6 close, 11/4 = 3
  -- far), so the boundary is directly observable.  Swept 61→87 px in 2 px
  -- steps against a guarding Reptile [mk2.md "The bot's own spacing bands"]:
  --   61, 63 px -> chip 6  (CLOSE HP)      65 .. 81 px -> chip 3 (FAR HP)
  -- which narrows mk2.md's earlier "between 63 and 69" bracket to **between
  -- 63 and 65**.
  close_gap_max   = 63,   -- <= this is measured CLOSE (chip 6 at 61 and 63)
  far_gap_min     = 65,   -- >= this is measured FAR   (chip 3 from 65 up)
  jab_gap_max     = 81,   -- last gap where far HP CONNECTED in that sweep.
                          -- The table's connect_range for the row is 83
                          -- [rows 21/22]; the sweep whiffed at 83, so the bot
                          -- takes the conservative of the two.
  commit_gap_max  = 99,   -- `mileena/HK/far` connect_range = 99 px [rows 29-34]
  ender_gap_max   = 83,   -- `mileena/cLK` connect_range = 83 px    [rows 65-70]
  -- ── the ender, REFUSED (a range/timing clause, not a correction) ────────
  -- The design wanted cLK as the safe turn-ender: −2 on block [rows 65-70],
  -- and its +32 px blocked pushback puts Reptile's close HP out of range
  -- [mk2.md "Range clauses that neutralise a published negative number"].  It
  -- is NOT REACHABLE from +13 jab pressure on this port, measured:
  --   * cLK needs the crouch stance ESTABLISHED before the button, and the
  --     stance clock starts at actionability, not when Down is first held —
  --     holding Down through the jab's recovery buys nothing (three runs, the
  --     game produced a standing far LK, 26 damage, every time).
  --   * swept from the jab's own earliest free frame f19, 2 reps per cell,
  --     deterministic: lead 6,10,12,14 -> NOTHING; lead 8 -> a close LK (chip
  --     4); lead >= 15 -> cLK (chip 2).
  --   * so the button lands at f19+15 = **f34**, and the opponent's earliest
  --     press after a blocked far HP is **f32** [mk2.md attack-press rig].
  --     The ender is 2 frames late: the +13 is spent before it comes out.
  -- The bot therefore ends its string by RE-GUARDING, which the +13 pays for
  -- comfortably.  Set true to run the (measurably worse) ender arm.
  ender_enabled   = false,
  punish_gap_max  = 63,   -- close HP is the punisher; chip 6 (=24 dmg) at 63
                          -- [row 23 + the sweep above]
  -- NOT A MEASUREMENT — an engineering margin, named as such.  connect_range
  -- is the maximum reach of a move that CONNECTS from a standstill; both
  -- fighters drift (blocked contact slides them apart for ~10 frames — see
  -- `chain_gap_max`), so a jab thrown at the very edge is a coin flip.  A
  -- whiff is handled safely (the bot re-guards), so this only trades pressure
  -- density for fewer whiffs.
  reach_margin    = 3,

  -- ── the block-stance latch ──────────────────────────────────────────────
  -- A press inside the latch is EATEN: nothing comes out, and it measures
  -- exactly like a whiff.  mk2.md's W1 section bracketed it as "release-gap 7
  -- fails / 8 succeeds for every hold >= 15; the one short hold measured, 8
  -- frames, needed 10", and src/training.rs ships PUNISH_RELEASE = 12 on that.
  -- 12 IS NOT ENOUGH FOR THIS BOT and the first build of it whiffed 13/13
  -- jabs because of it: the bot's neutral guard is often only 1-2 frames long,
  -- and a SHORT hold has a LONGER latch.  Re-measured on m-gap-39, 2 reps per
  -- cell, every cell deterministic [mk2.md "The block latch is a stance
  -- LIFETIME, not a release tail"]:
  --   hold  1f -> 17   hold  2f -> 16   hold  4f -> 14   hold  6f -> 12
  --   hold  8f -> 10   hold 10f ->  8   hold 12f ->  8   hold 15f ->  8
  -- which is exactly  min_gap = max(8, 18 - hold)  — i.e. the stance has a
  -- ~18-frame minimum LIFETIME from the frame Block goes down, plus an
  -- 8-frame tail after it comes up.  (The prior 8->10 and >=15->8 cells
  -- reproduce exactly; this only extends the bracket downward.)  Identical
  -- for HP and HK, so unlike the same-frame eat it is NOT per-button.
  latch_life = 18,   -- frames from Block PRESS before an attack can land
  latch_tail = 8,    -- frames from Block RELEASE before an attack can land

  -- ── neutral / turn hand-off ─────────────────────────────────────────────
  -- Quiet frames with no further damage before the bot calls the opponent's
  -- turn over.  20 = the profile's `framelab.quiet_frames` / calibration
  -- HITSTUN_RECENT_FRAMES, the same window src/training.rs uses to decide a
  -- string has ended.
  defend_quiet   = 20,
  -- A string that ENDS hands the turn over, and the opponent's answer takes
  -- time to arrive — so "nobody has hurt me yet" is NOT evidence the turn is
  -- back.  The first build left neutral one frame after entering it and ate
  -- three clean reversals in 500 frames; every one of them landed inside
  -- `defend_quiet` frames of the string ending.  The floor is therefore
  --   defend_quiet + max(0, -on_block of the move that ended the string)
  -- — the second term is the measured number of frames the opponent is free
  -- before the bot is (e.g. 20 for a blocked far HK, row 33/34).
  guard_after_string = true,
  -- NOT A MEASUREMENT — a behaviour knob.  How many blocked-but-PLUS actions
  -- the bot chains together before it voluntarily hands the turn back.  The
  -- +13 loop is genuinely unbreakable by a dummy that only reverses, so
  -- without a cap the bot never gives the opponent a turn at all (measured: 1
  -- hand-off in 2400 frames).  3 keeps the fight a fight.
  max_string_actions = 3,
  -- NOT A MEASUREMENT — a behaviour knob.  How long the bot stops trying to
  -- punish after a punish came back blocked, so it leaves the collision floor
  -- and returns to the far band.  120 frames ≈ 2.2 s at this core's 54.71 Hz.
  punish_cooldown = 120,
  -- Hard cap so a missed exit can never deadlock the fight (acceptance test
  -- 3).  3 s of frames; nothing measured picks this, it is a watchdog.
  defend_max     = 180,

  -- ── confirm window ──────────────────────────────────────────────────────
  -- far HP contacts at press+11 and Mileena's own earliest free re-press is
  -- press+19 [mk2.md "A second RIG for advantage", `mileena/HP/far` sweep:
  -- attacker earliest f19, f15-f18 dead].  So the classification has 8 frames
  -- of slack between the contact frame and the frame it must act on — that is
  -- the honest confirm window, and it is measured, not assumed.
  confirm_min_t  = 4,     -- no normal here contacts sooner than press+8
  confirm_max_t  = 18,    -- must be classified before the f19 decision

  -- The measured CHAIN window on far HP: a second far HP pressed at f12/f13/
  -- f14 after a blocked first one comes out IMMEDIATELY (contact f23/f24/f25);
  -- f15-f18 produce nothing at all [mk2.md "A punch CHAIN window on far HP"].
  chain_enabled  = true,
  chain_press_t  = 12,
  chain_max      = 1,     -- one chained jab: blocked-jab pushback is +9 px
                          -- each (71 -> 80 -> 89) and connect_range is 83, so
                          -- a third jab whiffs by measurement [mk2.md
                          -- "Blocked-contact pushback"].
  -- The chain has a SHORTER reach than the jab that opened the string, and it
  -- is not the move's fault: blocked-contact pushback is a ~9 px SLIDE spread
  -- over the following ~10 frames (per-frame trace: 71,71,72,73,74,74,75,76,
  -- 77,77,78,79 …), so the chain, pressed at t=12 and contacting ~10 frames
  -- later, lands about 9 px further out than where it was aimed.  Swept
  -- directly [mk2.md "The bot's own spacing bands"]: chain CONNECTS from 65 to
  -- 73 px, WHIFFS from 75 px up — 8 px inside the jab's own 81.
  chain_gap_max  = 73,

  -- ── the commit ──────────────────────────────────────────────────────────
  -- far HK: +3 on hit, −20 on block [rows 29-34].  It cannot be taken back,
  -- so it is thrown ONLY on a confirmed HIT.  Set false to run the control
  -- arm (a bot that never commits).
  commit_enabled = true,

  -- ── trace ───────────────────────────────────────────────────────────────
  trace_frames   = true,  -- per-frame samples (for the turn timeline)
  trace_cap      = 24000, -- ring cap; ~6.7 minutes of frames
  overlay        = true,  -- on-screen state readout (GUI + MCP screenshots)
}

-- ═══════════════════ MOVES: the measured action scripts ═════════════════════
-- `press`   : the button table held for `hold` frames from t=0
-- `free`    : Mileena's own earliest free re-press, frames after t=0 — the
--             frame the bot is allowed to start its next action on.
-- `hit`     : struct-health damage when it CONNECTS (the confirm discriminator)
-- Blocked contact chips a quarter of that (3/6/8 on this port — the profile's
-- `_STATUS` and mk2.md "Hitstun / blockstun observables"), so the bot
-- classifies by AMOUNT: >= `hit` is a hit, anything smaller but nonzero is a
-- block, nothing at all by `confirm_max_t` is a whiff.
-- Button names are the profile's `attack_chords`: HP=y, LP=b, HK=x, LK=a,
-- Block=l.
local MOVES = {
  jab_far = {                              -- mileena/HP/far  [rows 21/22]
    label = "HP/far", press = {y=true}, hold = 3,
    free = 19,     -- attacker earliest re-press f19 [mk2.md attack-press rig]
    hit  = 11,     -- 11 damage far / chip 3
    adv_block = 13, adv_hit = 4,
  },
  jab_close = {                            -- mileena/HP/close [rows 23/24]
    label = "HP/close", press = {y=true}, hold = 3,
    free = 19,     -- no separate close-HP sweep exists; far HP's f19 is used
                   -- and flagged in the report as a DATA GAP.
    hit  = 24,     -- 24 damage close / chip 6
    adv_block = -2, adv_hit = 25,
  },
  commit = {                               -- mileena/HK/far  [rows 29-34]
    label = "HK/far", press = {x=true}, hold = 3,
    free = 49,     -- attacker earliest re-press f49 on BLOCK [mk2.md]; the
                   -- on-HIT absolute was never measured, so the blocked (and
                   -- therefore slower) number is used for both — see report.
    hit  = 32,     -- 32 damage / chip 8
    adv_block = -20, adv_hit = 3,
  },
  ender = {                                -- mileena/cLK     [rows 65-70]
    label = "cLK", press = {a=true, down=true}, hold = 3,
    -- THE CROUCH STANCE HAS A LEAD-IN.  Pressing Down+LK together does NOT
    -- produce cLK: measured on m-gap-39, 2 reps per cell, every cell
    -- deterministic [mk2.md "The crouch stance needs a 6-frame lead-in"] —
    --   lead 0,1,2,3,4,5 frames -> NOTHING comes out (0 damage)
    --   lead 6,7,8,12   frames -> cLK,  6 damage on hit / 2 chip on block
    -- The first build held them together and the game gave it a standing FAR
    -- LK instead (26 damage, `mileena/LK/far`, rows 43/44 — a −20-on-block,
    -- −25-on-hit move) while the bot's log said "cLK, −2".  That is the exact
    -- shape of CLAUDE.md's "a stance lead-in" scaling law: it is per subject,
    -- and it has to be measured, not assumed.
    -- The 2-damage chip is also new: the profile's `_STATUS` says blocked
    -- normals "always chip 3/6/8 on this port"; cLK chips 2.
    lead = {down=true}, lead_frames = 15,   -- see CONFIG.ender_enabled
    free = 39,     -- attacker earliest re-press f39 AFTER THE BUTTON
                   -- [mk2.md m-gap-45 sweep], so the whole action is
                   -- lead_frames + free long.
    hit  = 6,      -- 6 damage on hit, 2 chip on block
    adv_block = -2, adv_hit = -11,
  },
}

-- ══════════════════════════════ BOT STATE ═══════════════════════════════════

BOT = {
  frame        = -1,      -- last emulated frame we ran the tick on
  state        = "boot",
  state_since  = 0,
  act          = nil,     -- {move=<MOVES entry>, key=, t0=, class=, delta=}
  chain_n      = 0,       -- chained jabs in the current string
  string_n     = 0,       -- actions spent in the current pressure string
  block_down_since = nil, -- frame the current continuous Block hold started
  latch_ok_at  = -10000,  -- earliest frame an attack press can actually land
                          -- (the block-stance latch; see CONFIG.latch_*)
  guard_until  = 0,       -- earliest frame the bot may LEAVE neutral
  hp = {nil, nil},        -- last seen struct health, per block
  last_hurt    = -10000,  -- frame our OWN health last decreased
  last_dealt   = -10000,  -- frame the OPPONENT's health last decreased
  gap          = nil,     -- last resolved gap (px); nil = ABSENT this frame
  punish_ready = false,   -- absorbed something on guard -> close HP is live
  punish_blocked_until = 0, -- punish cooldown after a blocked punish
  absent_gap   = 0,       -- consecutive frames x could not be resolved
  facing_right = true,    -- forward = right?
  events       = {},      -- decision log
  samples      = {},      -- per-frame trace
  counters     = {
    jabs = 0, chains = 0, commits = 0, commits_refused = 0, enders = 0,
    punishes = 0, hits = 0, blocks = 0, whiffs = 0,
    taken_clean = 0, taken_chip = 0,
  },
}

-- Ring trim, done in BLOCKS: `table.remove(t, 1)` every frame is O(n) per
-- frame once the cap is reached, which on a long fight is the script's whole
-- cost.  Halving amortises it to O(1) per frame.
local function trim_ring(t)
  if #t <= CONFIG.trace_cap then return t end
  local keep, out = math.floor(CONFIG.trace_cap / 2), {}
  for i = #t - keep + 1, #t do out[#out+1] = t[i] end
  return out
end

local function log(kind, tbl)
  local parts = {"f=" .. tostring(BOT.frame), "ev=" .. kind}
  if tbl then
    -- Stable key order so the trace diffs cleanly between runs.
    local keys = {}
    for k in pairs(tbl) do keys[#keys+1] = k end
    table.sort(keys)
    for _, k in ipairs(keys) do
      parts[#parts+1] = k .. "=" .. tostring(tbl[k])
    end
  end
  BOT.events[#BOT.events+1] = table.concat(parts, " ")
  BOT.events = trim_ring(BOT.events)
end

-- `hold_extra` (frames) is the measured disadvantage the string ended on —
-- see CONFIG.guard_after_string.
local function goto_state(s, why, hold_extra)
  if BOT.state == s then return end
  log("state", {from = BOT.state, to = s, why = why or "-"})
  BOT.state = s
  BOT.state_since = BOT.frame
  -- Entering neutral always ends the string: no action is in flight, so the
  -- confirm classifier has nothing stale to resolve against.
  if s == "neutral" then
    BOT.act = nil
    BOT.string_n = 0                    -- the turn ended; a new one starts fresh
    local extra = CONFIG.guard_after_string and (hold_extra or 0) or 0
    BOT.guard_until = BOT.frame + CONFIG.defend_quiet + extra
  end
end

-- Frames spent in the current state, measured on the frame we are DECIDING
-- for (framecount+1), because a hold issued in this callback lands next frame.
local function in_state() return (BOT.frame + 1) - BOT.state_since end

-- ═══════════════════════ OBSERVATION (reads only) ═══════════════════════════

-- Struct health for both fighters.  nil is ABSENT, never 0.
local function read_health()
  return game.read_field(1, "health"), game.read_field(2, "health")
end

-- Gap in px from the object_ptr-resolved `x` of both fighters.  Returns nil
-- when EITHER side's pointer is stale this frame (docs/frames.md §5 / the
-- CLAUDE.md gotcha): the bot then declines to make a spacing decision rather
-- than acting on a synthesized 0.
local function read_gap()
  local x1 = game.read_field(1, "x")
  local x2 = game.read_field(2, "x")
  if not x1 or not x2 then return nil, nil end
  local d = x2 - x1
  return (d < 0) and -d or d, (d >= 0)
end

-- ═════════════════════════ ACTION EXECUTION ═════════════════════════════════

-- Start `key` on the NEXT emulated frame.  t=0 is that frame — the same zero
-- the frame table's "press at fN" convention uses.
local function start_action(key, why)
  local mv = MOVES[key]
  BOT.act = {move = mv, key = key, t0 = BOT.frame + 1, class = nil, delta = 0}
  log("press", {move = mv.label, why = why or "-", gap = BOT.gap or "absent"})
  local c = BOT.counters
  if key == "jab_far" or key == "jab_close" then c.jabs = c.jabs + 1
  elseif key == "commit" then c.commits = c.commits + 1
  elseif key == "ender" then c.enders = c.enders + 1 end
end

-- Buttons for the action's frame `t`.  An action is
--   [ lead_frames of `lead` ] [ hold frames of `press` ] [ nothing ]
-- The lead is the STANCE the move needs established before the button
-- (crouching, for cLK — see MOVES.ender).  Moves with no stance requirement
-- have lead_frames = 0 and the press starts at t = 0, which is the frame the
-- measured tables call f0.
local function lead_of(mv) return mv.lead_frames or 0 end

local function action_bits(act, t)
  local mv = act.move
  local lead = lead_of(mv)
  local b = {}
  if t < lead then
    for k, v in pairs(mv.lead) do b[k] = v end
    return b
  end
  if t < lead + mv.hold then
    for k, v in pairs(mv.lead or {}) do b[k] = v end
    for k, v in pairs(mv.press) do b[k] = v end
    return b
  end
  return {}
end

-- ═══════════════════════════ THE POLICY ═════════════════════════════════════

-- Is a close-HP PUNISH the right opener right now?  Only when the bot has
-- actually absorbed something on guard (so the opponent is committed) and the
-- gap is inside close HP's measured 24-damage range.
local function punish_window(gap)
  return BOT.punish_ready and gap and gap <= CONFIG.punish_gap_max
     and BOT.frame >= (BOT.punish_blocked_until or 0)
end

-- Which starter does this gap support?  Returns key, or nil + a reason.
local function pick_starter(gap)
  if not gap then return nil, "gap-absent" end
  if punish_window(gap) then return "jab_close", "punish" end
  if gap < CONFIG.far_gap_min then return nil, "too-close" end
  if gap <= CONFIG.jab_gap_max - CONFIG.reach_margin then return "jab_far", "far" end
  return nil, "out-of-range"
end

-- Walk bits toward/away, or {} when already in the band.
local function approach_bits(gap)
  if not gap then return {} end
  local fwd = BOT.facing_right and "right" or "left"
  local back = BOT.facing_right and "left" or "right"
  if punish_window(gap) then return {} end      -- standing still to punish
  if gap > CONFIG.jab_gap_max - CONFIG.reach_margin then
    return {[fwd] = true}                       -- 3.0 px/frame [mk2.md item 3]
  elseif gap < CONFIG.far_gap_min then
    return {[back] = true}                      -- 2.0 px/frame; back OUT of
                                                -- close range, where HP is a
                                                -- −2 punisher and not a
                                                -- +13 pressure tool
  end
  return {}
end

-- The whole decision for one frame.  Returns the button table to hold.
local function decide()
  local gap = BOT.gap
  local st  = BOT.state

  -- ── S_NEUTRAL: the opponent's turn.  Hold Block. ──────────────────────
  if st == "neutral" then
    local quiet = BOT.frame - BOT.last_hurt
    local stuck = in_state() >= CONFIG.defend_max
    if (quiet >= CONFIG.defend_quiet and BOT.frame >= (BOT.guard_until or 0))
       or stuck then
      goto_state("approach", stuck and "defend-watchdog" or "opponent-quiet")
      return approach_bits(gap)
    end
    return {l = true}
  end

  -- ── S_APPROACH: Block is off; walk into the band, wait out the latch ──
  if st == "approach" then
    local bits = approach_bits(gap)
    if (BOT.frame + 1) < BOT.latch_ok_at then
      return bits                               -- latch: a press here is EATEN
    end
    local key, why = pick_starter(gap)
    if key then
      BOT.chain_n = 0
      if why == "punish" then
        BOT.counters.punishes = BOT.counters.punishes + 1
      end
      BOT.punish_ready = false                  -- one window, one attempt
      goto_state("act", "string-start")
      start_action(key, why)
      return action_bits(BOT.act, 0)
    end
    -- In range for nothing and not walking anywhere useful: the punish window
    -- has expired (the opponent is out of close-HP range).  Drop it so the
    -- bot does not stand still waiting for a punish it can never land.
    if BOT.punish_ready and gap and gap > CONFIG.punish_gap_max then
      BOT.punish_ready = false
      log("punish_expired", {gap = gap})
    end
    return bits
  end

  -- ── S_ACT: executing a measured action script ─────────────────────────
  if st == "act" and BOT.act then
    local act = BOT.act
    local t   = (BOT.frame + 1) - act.t0

    -- (1) The chain window, only on a CONFIRMED BLOCK of a far jab.
    if CONFIG.chain_enabled and act.class == "block" and act.key == "jab_far"
       and t == CONFIG.chain_press_t and BOT.chain_n < CONFIG.chain_max
       and gap and gap <= CONFIG.chain_gap_max and gap >= CONFIG.far_gap_min then
      BOT.chain_n = BOT.chain_n + 1
      BOT.counters.chains = BOT.counters.chains + 1
      start_action("jab_far", "chain@" .. t)
      return action_bits(BOT.act, 0)
    end

    -- (2) Still inside the action's own recovery.  Nothing may be scheduled
    -- to overlap it: a stance lead-in CANNOT be hidden inside another move's
    -- recovery (starting the ender early so its LK press landed exactly on
    -- f19 was tried, and the game produced a standing far LK — 26 damage,
    -- three runs).  See CONFIG.ender_enabled.
    if t < lead_of(act.move) + act.move.free then
      -- A commit that came back BLOCKED is −20: start guarding immediately.
      -- Guard returns before the walk does (docs/frames.md §1), so holding
      -- Block through the recovery is strictly better than standing.
      if act.key == "commit" and act.class == "block" then
        goto_state("neutral", "commit-blocked", -act.move.adv_block)
        return {l = true}
      end
      return action_bits(act, t)
    end

    -- (3) t == free: the action is over.  Decide the next one.
    local cls = act.class or "whiff"
    -- How many frames the opponent is free before we are, if that move was
    -- blocked — the measured `on_block` of the move we just threw.
    local minus = (cls == "block") and math.max(0, -act.move.adv_block) or 0

    if act.key == "commit" then
      if cls == "hit" then
        -- +3: the turn is still ours, so do NOT re-hold Block (that would
        -- cost another latch wait).  Go straight back to spacing.
        goto_state("approach", "commit-hit")
        return approach_bits(gap)
      end
      goto_state("neutral", "commit-" .. cls, minus)
      return {l = true}
    end

    if act.key == "ender" then
      goto_state("neutral", "ender-" .. cls, minus)
      return {l = true}
    end

    -- A jab resolved.
    if cls == "hit" then
      if CONFIG.commit_enabled and gap and gap >= CONFIG.far_gap_min
         and gap <= CONFIG.commit_gap_max then
        start_action("commit", "confirmed-hit")
        return action_bits(BOT.act, 0)
      end
      -- Confirmed, but the commit's own measured connect range does not
      -- cover this gap (or commits are disabled) — refuse it, keep jabbing.
      BOT.counters.commits_refused = BOT.counters.commits_refused + 1
      log("commit_refused", {
        reason = (not CONFIG.commit_enabled) and "disabled"
                 or (gap and "range" or "gap-absent"),
        gap = gap or "absent"})
      local k = pick_starter(gap)
      if k then start_action(k, "hit-no-commit"); return action_bits(BOT.act, 0) end
      goto_state("approach", "hit-respace")
      return approach_bits(gap)
    end

    if cls == "block" then
      -- THE TURN RULE, straight off the table: continue the string only while
      -- the blocked move left us PLUS.  far HP is +13 (rows 21/22), so the
      -- turn did not pass and the bot keeps it.  close HP is −2 (row 23): the
      -- turn DID pass, so the bot guards instead of pressing on.  (Ignoring
      -- this cost the first build both of its clean punishes taken — it
      -- answered a blocked −2 close HP with another attack and got hit out of
      -- the startup.)
      if act.key == "jab_close" then
        -- A punish that came back BLOCKED was not a punish: the opponent is
        -- guarding, and close HP is −2 there (row 23).  Stop re-arming it for
        -- a while so the bot walks back out to its +13 far game instead of
        -- looping close-HP into a guard forever (observed: 14 straight
        -- close-HP/chip cycles at the 62 px collision floor).
        BOT.punish_blocked_until = BOT.frame + CONFIG.punish_cooldown
      end
      if CONFIG.ender_enabled and act.move.adv_block > 0
         and gap and gap <= CONFIG.ender_gap_max then
        start_action("ender", "blocked-string-end")
        return action_bits(BOT.act, 0)
      end
      -- SPEND THE PLUS ON THE APPROACH.  A blocked far jab is +13 and pushes
      -- the opponent 9 px out of range, so the correct answer is neither "jab
      -- again from here" (it whiffs) nor "guard" (that costs a fresh 8-18
      -- frame block-stance latch to get out of again — the single most
      -- exposed thing the bot ever does).  It is to WALK IN on the +13:
      -- 13 free frames × 3.0 px/frame [mk2.md item 3] buys 39 px, the gap
      -- only opened by 9, and the bot's next jab presses ~f22 against an
      -- opponent whose own earliest press is f32.  Same currency as the
      -- knockdown in mk2.md's "Slide knockdown" section: the free APPROACH.
      if act.move.adv_block > 0 and BOT.string_n < CONFIG.max_string_actions then
        BOT.string_n = BOT.string_n + 1
        goto_state("approach", "plus-continue")
        return approach_bits(gap)
      end
      goto_state("neutral",
        (act.move.adv_block > 0) and "blocked-plus-reguard" or "blocked-minus",
        minus)
      return {l = true}
    end

    -- Whiff: nothing connected.  A whiff is exposure — re-block.
    goto_state("neutral", "whiff")
    return {l = true}
  end

  -- Boot / unknown: guard.
  goto_state("neutral", "boot")
  return {l = true}
end

-- ══════════════════════════════ THE TICK ════════════════════════════════════

local function tick()
  local f = emu.framecount()
  if f == BOT.frame then return end            -- GUI/paused frames: no decision
  BOT.frame = f

  if not CONFIG.enabled then return end

  -- ── observe ───────────────────────────────────────────────────────────
  local h1, h2 = read_health()
  local gap, right = read_gap()
  BOT.gap = gap
  if gap then
    BOT.absent_gap = 0
    BOT.facing_right = right
  else
    BOT.absent_gap = BOT.absent_gap + 1
  end

  local controllable = game.controllable()

  -- Health deltas, DECREASE ONLY (the profile's contact_signal direction):
  -- immune by one sign check to the training refill's write back to max and
  -- to the round-intro ramp.
  local d_me, d_opp = 0, 0
  if h1 and BOT.hp[1] and h1 < BOT.hp[1] then d_me  = BOT.hp[1] - h1 end
  if h2 and BOT.hp[2] and h2 < BOT.hp[2] then d_opp = BOT.hp[2] - h2 end
  if h1 then BOT.hp[1] = h1 end
  if h2 then BOT.hp[2] = h2 end

  -- ── our own damage: whose turn it is just changed ──────────────────────
  if d_me > 0 then
    BOT.last_hurt = f
    local blocking = (BOT.state == "neutral")
    -- Blocked contact chips a quarter; a full-damage step while NOT guarding
    -- is a CLEAN hit taken.  Both are logged with the amount, so the
    -- validation script classifies from the number, not from this label.
    if blocking then
      BOT.counters.taken_chip = BOT.counters.taken_chip + 1
      -- Absorbed on guard => the opponent is COMMITTED to something.  Arm the
      -- punish; it fires once the guard window closes and the block-stance
      -- latch clears (CONFIG.latch_*), which is the earliest a counter can
      -- physically come out of a held guard on this port.
      BOT.punish_ready = true
    else
      BOT.counters.taken_clean = BOT.counters.taken_clean + 1
    end
    log("hurt", {dmg = d_me, guarding = blocking, state = BOT.state,
                 punish_state = training.punish_state(), gap = gap or "absent"})
    if BOT.state ~= "neutral" then
      BOT.act = nil
      goto_state("neutral", "hurt")
    else
      BOT.state_since = f                       -- refresh the guard window
      BOT.guard_until = f + CONFIG.defend_quiet
    end
  end

  -- ── the CONFIRM: classify our in-flight action by the DELTA AMOUNT ─────
  if BOT.act and not BOT.act.class then
    -- Frames since the BUTTON (not since the action started): a move with a
    -- stance lead-in presses `lead_frames` later than it begins.
    local t = f - BOT.act.t0 - lead_of(BOT.act.move)
    if d_opp > 0 and t >= CONFIG.confirm_min_t then
      BOT.last_dealt = f
      BOT.act.delta = d_opp
      BOT.act.class = (d_opp >= BOT.act.move.hit) and "hit" or "block"
      local c = BOT.counters
      if BOT.act.class == "hit" then c.hits = c.hits + 1 else c.blocks = c.blocks + 1 end
      log("confirm", {move = BOT.act.move.label, t = t, dmg = d_opp,
                      expect_hit = BOT.act.move.hit, class = BOT.act.class,
                      gap = gap or "absent"})
    elseif t >= CONFIG.confirm_max_t then
      BOT.act.class = "whiff"
      BOT.counters.whiffs = BOT.counters.whiffs + 1
      log("confirm", {move = BOT.act.move.label, t = t, dmg = 0,
                      expect_hit = BOT.act.move.hit, class = "whiff",
                      gap = gap or "absent"})
    end
  end

  -- ── decide ────────────────────────────────────────────────────────────
  local bits
  if not controllable then
    -- Gate shut (round banner, menu, round over): hands off entirely, and
    -- drop any in-flight action so the next fight starts clean.
    BOT.act = nil
    if BOT.state ~= "gate" then goto_state("gate", "not-controllable") end
    bits = {}
  else
    if BOT.state == "gate" then goto_state("neutral", "gate-open") end
    bits = decide()
  end
  input.hold(0, bits)

  -- ── block-stance latch bookkeeping ────────────────────────────────────
  -- `bits` lands on frame f+1, so that is the frame Block goes down/up on.
  -- On release, the earliest frame an attack press can actually come out is
  -- max(latch_tail after release, latch_life after the press) — the measured
  -- max(8, 18 - hold) rule, expressed as two independent floors.
  local blocking = bits.l and true or false
  if blocking and not BOT.block_down_since then
    BOT.block_down_since = f + 1
    BOT.latch_ok_at = 1 / 0                      -- unreachable while guarding
  elseif not blocking and BOT.block_down_since then
    local held = (f + 1) - BOT.block_down_since
    BOT.latch_ok_at = math.max((f + 1) + CONFIG.latch_tail,
                               BOT.block_down_since + CONFIG.latch_life)
    log("unguard", {held = held, press_ok_at = BOT.latch_ok_at,
                    wait = BOT.latch_ok_at - (f + 1)})
    BOT.block_down_since = nil
  end

  -- ── trace ─────────────────────────────────────────────────────────────
  if CONFIG.trace_frames then
    local held = {}
    for k in pairs(bits) do held[#held+1] = k end
    table.sort(held)
    BOT.samples[#BOT.samples+1] = table.concat({
      f, BOT.state, BOT.act and BOT.act.move.label or "-",
      BOT.act and (f + 1 - BOT.act.t0) or -1,
      BOT.act and (BOT.act.class or "-") or "-",
      h1 or "", h2 or "", gap or "", table.concat(held, "+"),
      controllable and 1 or 0, training.punish_state(),
    }, ",")
    BOT.samples = trim_ring(BOT.samples)
  end

  -- ── overlay ───────────────────────────────────────────────────────────
  if CONFIG.overlay then
    gui.text(4, 4, "CONFIRM BOT  " .. BOT.state, 0xFFFFFFFF)
    gui.text(4, 14, string.format("gap %s  act %s/%s",
      gap and string.format("%d", gap) or "ABSENT",
      BOT.act and BOT.act.move.label or "-",
      BOT.act and (BOT.act.class or "...") or "-"), 0xC8E6FFFF)
    local c = BOT.counters
    gui.text(4, 24, string.format("hit %d blk %d whf %d | cmt %d/%d | clean %d",
      c.hits, c.blocks, c.whiffs, c.commits, c.commits + c.commits_refused,
      c.taken_clean), 0xFFE08AFF)
  end
end

event.onframeend(tick)

-- ════════════════════════════ CONTROL SURFACE ═══════════════════════════════
-- Everything below is for MCP `run_lua` — the validation script's handles.

-- Configure the fight in one call.  `dummy` is any native dummy mode string
-- ("free" to hand P2 to a human), `guard` a GuardMode ("all"/"none"/
-- "after_first_hit"/"random"), `reversal` a ReversalTiming ("fast"/"late"/a
-- number/{min=,max=}).  All three are write-gated (`--training` or the MCP
-- `enable_writes` tool arms them).
function BOT.setup(dummy, guard, reversal)
  if not training.enabled() then training.set_enabled(true) end
  if dummy then training.set_dummy(dummy) end
  if guard then training.set_guard(guard) end
  if reversal then training.set_reversal(reversal) end
  return string.format("dummy=%s guard=%s reversal=%s training=%s",
    training.dummy(), training.guard_mode(), tostring(training.reversal()),
    tostring(training.enabled()))
end

-- Clear the trace and every counter; call right before a measured run so the
-- run's numbers are its own.  Does NOT touch CONFIG.
function BOT.reset()
  BOT.events, BOT.samples = {}, {}
  for k in pairs(BOT.counters) do BOT.counters[k] = 0 end
  BOT.act, BOT.chain_n = nil, 0
  BOT.hp = {nil, nil}
  BOT.last_hurt, BOT.last_dealt = -10000, -10000
  BOT.block_down_since, BOT.latch_ok_at = nil, -10000
  BOT.punish_ready, BOT.punish_blocked_until = false, 0
  BOT.state, BOT.state_since = "boot", BOT.frame
  goto_state("neutral", "reset")
  BOT.guard_until = 0
  -- Hand the pad back EMPTY, not guarding: BOT.reset() runs outside the tick
  -- (it is an MCP `run_lua` call), so a Block held here would never go through
  -- the latch bookkeeping below — and the very first jab of the run would be
  -- eaten by a stance the bot does not know it is in.  That is exactly how the
  -- first build lost its opening jab on every run.
  input.hold(0, {})
  return "reset"
end

function BOT.status()
  local c = BOT.counters
  return string.format(
    "frame=%d state=%s gap=%s act=%s/%s events=%d samples=%d "
 .. "jabs=%d chains=%d hits=%d blocks=%d whiffs=%d commits=%d refused=%d "
 .. "enders=%d clean_taken=%d chip_taken=%d",
    BOT.frame, BOT.state, tostring(BOT.gap),
    BOT.act and BOT.act.move.label or "-", BOT.act and (BOT.act.class or "-") or "-",
    #BOT.events, #BOT.samples,
    c.jabs, c.chains, c.hits, c.blocks, c.whiffs, c.commits, c.commits_refused,
    c.enders, c.taken_clean, c.taken_chip)
end

-- Chunked dump (run_lua returns one string; keep each call bounded).
-- kind = "events" | "samples".  1-based, inclusive.
function BOT.dump(kind, from, n)
  local src = (kind == "samples") and BOT.samples or BOT.events
  from = from or 1
  n = n or 400
  local out = {}
  for i = from, math.min(#src, from + n - 1) do out[#out+1] = src[i] end
  return table.concat(out, "\n")
end

function BOT.count(kind)
  return #((kind == "samples") and BOT.samples or BOT.events)
end

console.log("[confirm_bot] loaded — Mileena hit-confirm attacker on port 0. "
  .. "BOT.setup(dummy, guard, reversal) / BOT.reset() / BOT.status() / "
  .. "BOT.dump(kind, from, n)")
