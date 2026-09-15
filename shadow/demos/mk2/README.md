# MK2 Combat Demos — verified frame data in motion

Choreographed turn-taking demonstrations of verified frame data (`library/mk2/arcade.frames.json`). Each sequence is a two-character fight recorded as a timestamped input slot, replayed frame-exactly, and rendered to a captioned GIF. All claims are verified by live memory reads (contact register, struct-health damage, object pointers) against an identical control run — see `VERIFICATION.md` for the full evidence.

## Sequences

| # | Title | Claim (one-liner) | Arena | Slot | Frames | Duration | Citation |
|---|---|---|---|---|---|---|---|
| 1 | Jabs don't pass the turn | +13 on block means a blocked jab returns the turn to the attacker | `m-gap-39` | `demo-1-jabs-dont-pass-the-turn` | 99 | 1.81s | `mileena/HP/far → on_block +13` |
| 2 | The hit-confirm | Same button, same frame, same spacing — only Block decides between +3 and −20 | `m-gap-39` | `demo-2-the-hit-confirm` | 152 | 2.78s | `mileena/HK/far → on_hit +3, on_block −20` |
| 3 | Over-commit and pay | The roll pays (−34), the teleport does not (−25 BLOCKED at 153 px) | `m-gap-0` | `demo-3-over-commit-and-pay` | 192 | 3.51s | `mileena/roll → on_block −34; teleport → on_block −25` |
| 4 | Safe means unpunishable | One punisher, two moves: −2 on block is safe, −14 is lethal | `m-gap-45` | `demo-4-safe-means-unpunishable` | 162 | 2.96s | `mileena/cLK → on_block −2; close HK → on_block −14` |
| 5 | Knockdown currency | A knockdown buys 15 free frames (approach window), not okizeme | `m-gap-25` | `demo-5-knockdown-currency` | 132 | 2.41s | `reptile/slide → knockdown +15; jab pressure on the approach` |

## How to regenerate a GIF

Python API: `source shadow/train/.venv/bin/activate` then

```python
from shadow_train.mcpclient import McpClient
from shadow_train.capture import capture_slot, assemble_gif
import json

client = McpClient("http://127.0.0.1:4027/mcp")
beats = json.load(open("shadow/demos/mk2/1-jabs-dont-pass-the-turn.beats.json"))
captions = [...]  # see Python script in this directory for caption building

png_paths = capture_slot(client, slot_name="demo-1-jabs-dont-pass-the-turn",
                         state_path=beats["arena"], out_dir="/tmp/frames")
assemble_gif(png_paths, "1-jabs-dont-pass-the-turn.gif", scale=2, captions=captions)
```

Or CLI: `python -m shadow_train.capture demo --slot demo-1-jabs-dont-pass-the-turn --state shadow/arenas/mk2/m-gap-39.state --out 1-jabs-dont-pass-the-turn.gif`

## How to replay a slot live

In a running RustRetro session (Training panel or MCP):

```python
# Enable writes first
client.call("enable_writes")

# Pause, play slot, resume
client.call("load_state", path="shadow/arenas/mk2/m-gap-39.state", pause_after=True)
client.call("play_inputs", action="start", name="demo-1-jabs-dont-pass-the-turn",
            port="both", trigger="manual")
client.call("resume")
```

Or graphically: **Training panel** → Record section → Load input slot dropdown → select `demo-N-*` → **Play** button.

## Geometry refusals (not corrections)

These are differences between **what the sequence executes** and **what a later punisher would do** — recorded because they are surprising and worth understanding:

1. **Sequence 3, teleport kick unpunishable**: Blocked teleport lands the attacker at a fixed 153 px gap (verified across 6 rungs). Reptile's longest connect range is 110 px (far HK/LK). The move is −25 on block, but the range law (docs/frames.md §1) forbids the punish. This is not a correction to −25; it is docs/frames.md §1's third clause in action.

2. **Sequence 5, knockdown "okizeme" refusal**: The slide knockdown buys +15 free frames at a 160 px launch distance. The sequence demonstrates free approach, not meaty pressure — Reptile cannot stand over Mileena on wakeup (he is ~77 px away when she acts). The claim is "approach window", not "okizeme", so no refusal is necessary; it is recorded here because the measuring rig initially expected contact and the distance is the surprising result.

See `VERIFICATION.md` for full methodology, controls, and cross-defender measurements.

---

# Tier 2: the emergent bot

The five sequences above are CHOREOGRAPHED — recorded input slots replayed
frame-exactly, identical every run. **`library/mk2/confirm_bot.lua` is the
same frame data as a live policy**: Mileena on P1, reacting to what the
opponent actually does, so the turn-taking EMERGES and no two runs are the
same. It is also the thing you can fight yourself.

## The one idea

Far HK is the turn decision and it cannot be un-committed: **+3 on hit, −20
on block** (`mileena/HK/far`, rows 29–34). So the bot never throws it on a
hunch. It throws the safe starter first — far HP, **+13 on block / +4 on
hit** (rows 21/22) — and classifies the outcome from the opponent's
**struct-health delta AMOUNT** on the contact frame: **11 = hit, 3 = chip
(blocked), 0 by f18 = whiff**. Only an 11 buys the commit. Contact lands at
f11 and Mileena's own earliest free re-press is f19, so the confirm has
**8 measured frames** of slack — the same window a human uses.

Blocked instead? The +13 means the turn did not pass, so the bot chains one
more jab inside the measured **f12–f14** window and then spends the rest of
the plus WALKING BACK INTO RANGE (blocked pushback is +9 px; 13 free frames ×
3.0 px/frame buys 39). After `max_string_actions` it hands the turn back and
guards. Hit by anything? It guards until the opponent is quiet, then answers
with close HP (24 damage) — but only once the **block-stance latch** has
cleared, which is the slowest thing in the whole design.

## Run it

Bot vs the training dummy, headless:

```sh
./target/release-dev/rustretro \
  --core ../FBNeo/src/burner/libretro/fbneo_libretro.dylib \
  --rom ~/games/roms/mk2.zip --game library/mk2 \
  --headless --mcp-port 4026 --pace 0 \
  --training --script library/mk2/confirm_bot.lua
```

then over MCP: `load_state path=shadow/arenas/mk2/m-gap-39.state` and
`run_lua script="BOT.setup('block_punish','all','fast')"`.

**Fight it yourself** — drop `--headless`, add `--scale 3`, and set the dummy
to Free (🎯 Training panel, or `run_lua script="BOT.setup('free')"`). You are
P2. The bot only ever holds port 0, so a second pad fights it directly.

It also loads into a session that is already running: paste the file through
MCP `run_lua` (verified — 40 KB in one call), or use the F10 script panel.
Tunables live in a `CONFIG` table at the top of the file and are re-read every
frame, so `run_lua script="CONFIG.commit_enabled = false"` takes effect on the
next frame without a reload (a reload would destroy the trace buffer).

## The acceptance runs

`python shadow/demos/mk2/validate_confirm_bot.py` (not a pytest test — it
needs the ROM, the core, and a live emulator; it launches its own on 4026).

**1 — the confirm.** Two families of 12 runs each from the same four arenas
(`m-gap-30/35/39/45`) × three phase offsets, differing in exactly one setting:
the dummy's guard mode. The dummy mode is pinned to plain `block` so nothing
else about P2 changes.

| family | jab deltas observed | runs | commits |
|---|---|---|---|
| `set_guard("all")` — BLOCKED | **{3}** only | 12 | **0**, in 0 runs |
| `set_guard("none")` — HIT | **{11}** only | 12 | **26**, in 12/12 runs |

The discriminator is the delta amount, and the commit follows it perfectly:
not one far HK in 12 blocked runs, at least two in every hit run.

**2 — safety under punishment.** BlockPunish dummy, `set_reversal("fast")`,
punish pool `{slide ×3, close HP ×2}`, 3 × 2400 frames:

| | m-gap-35 | m-gap-39 | m-gap-45 | total |
|---|---|---|---|---|
| contacts absorbed ON GUARD | 3 | 6 | 4 | 13 |
| clean hits taken | 2 | 1 | 5 | **8** |
| of those, real PUNISHES | 0 | 0 | 0 | **0** |

"Real punish" = a full-damage hit landing within |on_block| frames of one of
the bot's OWN blocked moves. All 8 clean hits were Reptile's slide (13
damage) landing ~10 frames after a blocked far HP — i.e. a slide already in
flight trading with the bot's walk-in, inside a move the bot was **+13** on.
That is not a punish, and the script says so rather than rounding it up.

**3 — turn alternation.** From the same runs, a timeline of who is making
contact: **10 / 14 / 17 hand-offs** per 2400-frame fight, longest stretch with
no contact by either side **2.7 s**. No deadlock.

## What the bot could not have, and why

Three constants the design wanted did not exist, and measuring them is most of
what this wave produced (`library/mk2/mk2.md`, "Four things the confirm bot
had to measure"):

1. **The block-stance latch is a stance LIFETIME, not a release tail**:
   `min_release_gap = max(8, 18 − block_hold_frames)`. The bot's first build
   whiffed 13/13 opening jabs because it guarded for 1 frame and then waited
   `src/training.rs`'s 12 — a 1-frame hold needs 17.
2. **The crouch stance needs a lead-in**, ~6 frames from idle and ~15 after a
   normal's recovery, and the clock starts at ACTIONABILITY, not when Down is
   first held. Pressing Down+LK together gives a standing far LK (26 damage,
   −20 on block) while your log happily says "cLK, −2".
3. **So cLK is not reachable as an ender**: its button lands at f34 after a
   blocked far HP, and the opponent's earliest press is f32. The −2 row is
   correct and simply out of reach — `CONFIG.ender_enabled = false`, with the
   refusal written next to it.

A recorded bot-vs-dummy session (16k frames, four arenas) is written to
`shadow/recordings/mk2/confirm-bot-*.jsonl` as segment-shadow feedstock;
recordings are gitignored.
