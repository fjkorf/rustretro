---
page:
  name: MatchupGrid
  label: "The Matchup Grid"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# The Matchup Grid

**What you'll do:** see which matchups you've actually demonstrated, jump straight to
a gap, and force the next fight to fill it.

## Reading the grid

Open the debugger and find the **🥊 Matchup** tab. Rows are characters you've played as
(`me`); columns are the full playable roster plus any extra ids you've fought (bosses).
Each cell is built from the `.rounds.jsonl` index sidecar the recorder writes next to
every recording:

- **`·`** (dim) — never demonstrated.
- **a number** — ≈decisions demonstrated for that matchup (frames ÷ 8, the decision
  cadence; demo rounds are excluded). Amber below roughly 1000 decisions — enough for
  the kNN to answer, nowhere near enough to capture the matchup; green above it.
- **`✓`** appended — a fitted model exists for this exact `(me, opp)` matchup.

If the grid is empty, recordings made from here on index themselves automatically;
backfill older ones with:

```bash
python -m shadow_train index                # index every shadow/recordings/*.jsonl
python -m shadow_train index --force         # rewrite sidecars that already exist
```

(run from `shadow/train/.venv`, or `shadow/train/.venv/bin/python3 -m shadow_train
index` from the repo root).

## Cell actions

Click a cell to select it. Below the grid you get:

- **round count + ≈decisions**, broken down by style tag if you tagged recordings.
- **Load model** (if a fitted model exists for this exact matchup) — loads it via the
  same mechanism as the 🎯 panel's picker.
- **Load arena** (if `shadow/arenas/<slug>.state` exists) — jumps straight into that
  matchup's saved position.
- If no model exists yet, the panel prints the fit command to run:
  `shadow/loop.sh --fit-only --me <me> --opp <opp>`.

## Forcing a matchup

Below the grid, **⚔ Force next fight vs `<opponent>`** freezes the game's
stage/opponent selector so every fight from here on — starting with the *next* one,
after the current one ends — is against that opponent on their home stage. You still
pick your own character normally; forcing only chooses the other side. The freeze
persists across fights (it re-writes the selector every frame, the same mechanism as a
frozen Watch) until you click **✕ Clear** — there is no "one fight only" mode. An amber
banner at the top of the panel shows which opponent is currently forced.

**Bosses** get their own quick-force row since they have no ladder-reachable grid
column until first fought — one click each to force that boss fight, or clear it.

(Force-matchup and the boss row only appear for games whose profile declares a
stage/opponent selector; a game without one hides this section entirely.)

## The coverage matrix CLI

The same information, without the debugger open:

```bash
python -m shadow_train coverage                       # scan shadow/recordings/*.jsonl
python -m shadow_train coverage session-1.jsonl ...    # scan specific files
```

Prints a text matrix of decisions-per-cell, useful for a quick terminal check or
scripting against (e.g. deciding what to record next in a headless session).

## Why it matters

A fixed-roster fighting game has a fixed number of matchups. The grid turns "which
matchups does the shadow actually know" from a guess into something you can see, click
into, and go fill — record the gap, fit it, check the grid again.

## See also

- [The Shadow Loop](/docs/tutorials/shadow-loop.md) — record → fit → fight, the loop
  this panel tracks coverage for.
- [Training Mode](/docs/tutorials/training-mode.md) — the panel this one sits next to.

<!-- litui:live
When litui is integrated, this page gains live embeds:
- [custom](matchup_grid_slot) the real 🥊 Matchup grid, clickable cells and all, as an
  escape hatch replacing the static description above
- [display] the currently forced matchup (if any) beside the "Forcing a matchup" section
Until then it renders as a static document page.
-->
