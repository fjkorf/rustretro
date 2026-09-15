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
