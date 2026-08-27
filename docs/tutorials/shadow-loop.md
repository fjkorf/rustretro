---
page:
  name: ShadowLoop
  label: "The Shadow Loop"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# The Shadow Loop

**What you'll do:** turn your own play into a behavioral-cloning opponent — record,
fit, fight — and repeat.

## The ritual

1. **Record.** With training mode on, hit **⏺ Start** in the 🎯 Training panel (or
   launch with `--record shadow/recordings/session-NAME.jsonl`). Play some rounds
   against a dummy or another human, then **⏹ Stop**. Recordings are jsonl-v2; the
   trainer skips demo rounds automatically (rounds with all-zero `p1_input`).

2. **Fit.** From the repo root:

   ```bash
   shadow/loop.sh                    # fit goat-vNEXT from the newest 12 v2 recordings
   shadow/loop.sh --fit-only         # same, but print the coverage report and stop
   shadow/loop.sh --me 1 --opp 7     # per-matchup fit, named by slug (goat-vs-rosemary)
   ```

   `--me`/`--opp` are character ids (see the roster table in
   `library/asurabld/asurabld.md`); filtering by either or both names the output model
   after `shadow_train.asurabld.matchup_slug` instead of the generic `goat-vN` series.
   Each run prints the recordings it used and the fitted model's coverage-drill report
   (which buckets are sparse and need more demonstration).

3. **Fight.** Three ways to load a fitted model into the running game:

   - **Shift+F5** toggles the already-loaded shadow on/off.
   - The 🎯 Training panel's model picker — pick one, or **Load ALL as set** to load
     every model under `shadow/models/` at once.
   - `shadow/loop.sh --push` loads the whole `shadow/models` set into the *running*
     app over MCP (`load_shadow`) instead of fighting over `shadow/play.py`. It always
     pushes the directory, not the single freshly-fit model, so a previously loaded set
     folds the new model in rather than being replaced.
   - `shadow/loop.sh --model NAME` skips fitting and fights an existing model directly
     over MCP via `shadow/play.py` (`--dry-run` there observes a live session without
     driving it).

   Fights read the arena from `ARENA`, or `shadow/arenas/current.state` if it exists
   (set from the 🎯 panel's Arena section), or `shadow/arenas/goat-vs-rosemary.state`
   otherwise.

## Model sets and auto matchup switching

A "set" is a directory of fitted models (`shadow/models/` itself, loaded via **Load ALL
as set** or `--push`). Loading a set keeps only the newest model per matchup key
`(me, opp)` — stale duplicates for the same matchup are dropped, not stacked. At every
round start the runner picks the best match for the current characters, in this
priority order: exact `(me, opp)` → per-`me` general → per-`opp` general → fully
general (`any, any`) → keep whatever was already active. So one `Load ALL as set` click
covers the general model plus every matchup-specific model you've fit, and the right
brain switches in automatically as the roster changes across a session.

## Family/port stamps and the cross-port warning

Every fitted model's `meta.json` carries the `family` (e.g. `asurabld`) and `port`
(e.g. `arcade`) it was trained on. Loading a model:

- with a **different family** is a hard error — it's the wrong game.
- with a **different port** (same family) loads fine but is stamped
  `[cross-port: <port>]` in the model card and logged as a warning — cross-port shadows
  are a supported experiment (e.g. training on one port, deploying against another),
  not an accident to hide.

## Where things live

`shadow/models/` and `shadow/recordings/` are gitignored — they're derived data, not
source (the `goat-v2` model directory is the one tracked exception, kept because the
native runner's tests read it). `shadow/arenas/current.state` is also gitignored
(machine-local); named arenas under `shadow/arenas/` are not.

## Why it matters

The loop is the whole point of the project: play well, record it, fit a model that
imitates it, then fight that model to find its gaps — which sends you back to step 1
with a specific matchup or bucket to demonstrate more of.

## See also

- [Training Mode](/docs/tutorials/training-mode.md) — the recorder and the panel this
  loop drives from.
- [The Matchup Grid](/docs/tutorials/matchup-grid.md) — see coverage across the whole
  roster and jump straight to a gap.

<!-- litui:live
When litui is integrated, this page gains live embeds:
- [display] the loaded shadow's model card (name, family/port stamp, cross-port
  warning) beside the "Family/port stamps" section
- [custom](shadow_section_slot) the 🎯 panel's Shadow bot section (model picker,
  Load ALL as set, enable toggle) as an escape hatch
Until then it renders as a static document page.
-->
