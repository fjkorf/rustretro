# Shadow — Updated Plan (2026-08-24)

Target: **(1) a training mode** and **(2) a trainable opponent** — a small model
that learns to play like the user from recorded demonstrations, improving as
more sessions accumulate.

This plan supersedes the wave sketch implied by `SPEC.md`/`IMPLEMENTATION.md`
and folds in: the 2026-08-24 live-grounding session (real health/timer
addresses, corrected fighter-block model, gamepad rig), and the four external
research reports (MAME driver + pugsy cheat DB, game-mechanics FAQs,
fbalpha2012 input source, and shipped player-cloning systems — KI Shadow AI,
Tekken Ghosts, slippi-ai, FightingICE). Findings live in
`library/asurabld/asurabld.md`; prior-art lessons summarized in §Design notes.

## Standing facts (what we now know)

- Attacks are RETRO **B/A/Y = Light/Medium/Heavy**; X/L/R never polled.
  Weapon toss = L+M+H chord; universal launcher = any-2 chord.
- Fighter data lives in two 0xDB4-stride blocks (`$403798`, `$40454C`):
  X/Y (+0x54/56), facing (+0x61), health pair (+0x177/179, max 0xEF, regen
  ~1%/1.5s standing), meter (+0x17B, per-char max +0x17F), char ID (+0x639).
  **The +0x28/2A hold accumulators track the OPPONENT's held direction** —
  never use them as self-features; derive holds from recorded input masks.
- Match control is writable: credits `$40655D`, finish-round `$400000`,
  round timer `$40000A/B` (BCD, freezable), stage `$40364D`.
- The recorder's current `controllable` gate is broken-permissive (true at
  title screen); the current p2 block (`$405300`) is dead data.
- Block→fighter assignment may vary per match/mode; anchor at round start
  (smaller X = left) + char ID + facing. 2P-VS layout unverified but the
  community FBNeo training mode uses the same addresses in VS free-play.

## Wave 2a — Ground-truth repair (recorder v2) — FIRST, blocks everything

1. **Block anchoring**: per-round resolver (X + char ID + facing) mapping
   block1/block2 → {me, opp}; store the mapping in each record row.
2. **Gate v2**: `controllable` = composite of known fields — both health
   pairs initialized AND round timer valid BCD AND hop flags clear. Validate
   across title/demo/char-select/intro/fight/KO/continue. No new discovery
   expected; fall back to a flag hunt only if the composite leaks.
3. **Recorder v2** (`src/record.rs`): record both blocks' full field set
   (x, y, facing, health pair, meter, char_id, action, anim, timer), input
   masks, round_id, block-anchor result. Drop `$405300`. Keep raw-on-disk.
4. **Micro-experiments** (sandbox, ~1 session): health-pair semantics (which
   byte is authoritative; 2-stacked-bars hypothesis; regen visibility),
   meter build observation from empty, facing during crossup frames.

Exit: recordings whose every field is trustworthy. ~1 session + small PR.

## Wave 2b — Training mode v1 (also the demonstration-recording rig)

- `--training` flag (or `training.lua`): auto-arm sandbox — write credits,
  freeze timer + healths (+ optionally meter), position-reset via X writes,
  round-reset via `$400000`, stage select.
- Dummy control presets on port-1 injection: stand / crouch / jump / hold-back
  (block-all) / record-replay input macros.
- Overlay (Lua `gui.draw*`): health/meter numeric, hitstun indicator via combo
  counters (`$4041E7`/`$40470B`), block-anchor debug readout.
- **Core-options support** (`RETRO_ENVIRONMENT_GET_VARIABLE`) — promoted from
  the roadmap: unlocks DIP-based free play, 1-round matches, damage scaling.
- **Save-states** (`retro_serialize`) — promoted: true state-slot resets;
  later the backbone of situation drills and replay.
- Reference: peon2/fbneo-training-mode `games/asurabld/asurabld.lua`.

Exit: one command drops the user into an infinite, resettable practice fight
with the fightstick; every minute of it is clean training data.

## Wave 2c — SPEC v2 amendments (doc pass, small but load-bearing)

- **Attack head → 6 classes**: `{None, L, M, H, Launcher(any-2), Toss(LMH)}`.
  Label extraction: ≥2 attack bits → Launcher unless all 3 → Toss.
- **Feature vector**: wire health (authoritative byte), meter, real facing,
  char IDs; DELETE RAM hold-accumulator features (opponent-linked); derive
  fwd/back-hold from input-mask history instead. Health features must
  tolerate upward drift (regen).
- **Humanness**: feed opponent state 2–4 frames stale; keep 8 Hz cadence.
- **Support constraint**: the sampler may only emit (state-bucket, action)
  pairs with nonzero empirical support — "everything it does, you did."
- Defense upweighting: oversample block/anti-air/escape transitions; report
  per-situation action distributions, not just global.
- Macro-action layer (deferred, designed now): recurring input strings from
  the command ring (`$400FD8`) become atomic replayable actions — the KI
  route to specials/EX, replacing the "higher-rate sub-head" idea.

## Wave 2d — Trainer + evaluation (new code, Python)

1. Dataset builder: JSONL → 8 Hz decisions (mode-of-window labels), chord
   classes, side-agnostic transform, K=4 stacking.
2. **kNN baseline first** (KI-style case retrieval over normalized states):
   cheap, cannot hallucinate, likely shippable as ghost v0.
3. Small MLP (two heads) with support-constrained temperature sampling;
   beat the kNN before keeping it.
4. Eval stack: held-out per-situation accuracy; conditional action-frequency
   match (neutral/offense/defense/oki buckets); A/B clip test with the user.
5. Coverage report: situations with < N examples (anti-air, wakeup, corner
   defense…) — doubles as the user-facing "what to demonstrate next".

## Wave 2e — Deploy (the ghost fights back)

1. **2P-VS verification session**: confirm block addresses/assignment in
   challenger mode (community evidence says same; one session to prove it).
2. Deploy harness: Python over MCP — read state, decide at 8 Hz, inject
   port-1 with held intents; reaction-delay built in. Latency budget is
   ample (125 ms slot vs ~10 ms roundtrips measured).
3. Later: in-app runner (Rust, or keep the sidecar — sidecar is fine for v1).

## Wave 2f — "Improves over time" loop (deliberately boring)

- Session flow: play (training mode or VS-ghost) → recordings append →
  retrain from scratch on a sliding window of recent sessions → new
  checkpoint = the ghost. Fixed architecture/temperature across sessions so
  progress reads as "it learned my new habit", not personality drift.
- Model registry: `shadow/models/<char>-<date>/` with calibration + coverage
  metadata. kNN case-store variant: append cases, cap by recency (KI: 40
  matches/matchup).

## Order & effort

| Wave | Depends on | Size |
|---|---|---|
| 2a recorder v2 | — | 1 session + small PR |
| 2b training mode v1 | 2a (uses same addresses) | 1–2 PRs (core-options & savestates separable) |
| 2c SPEC v2 | 2a findings | doc pass |
| 2d trainer | 2a data (2b accelerates collection) | new `shadow/train/` |
| 2e deploy | 2d + VS verification | 1 session + sidecar script |
| 2f loop | 2d/2e | thin glue |

Parallelizable: 2b's core-options/save-states are independent of 2a; 2c can
start immediately; 2d's dataset builder can develop against existing
imperfect recordings and swap in v2 data.

## Open risks

- Block-slot assignment rules unverified across modes/matches (2a #1 kills
  this); demo-era `$405300` third-slot question unresolved.
- Health-pair semantics assumption (authoritative vs display) pending 2a #4.
- 2P-VS layout: positive external evidence, no first-party proof until 2e #1.
- Stage-dependent speed-up quirk in some ROM versions — watch frame pacing
  in recordings.
- Specials/EX out of v1 scope by design; macro layer is the committed path.

## Post-plan addendum (2026-08-25) — the matchup layer

Waves 2a–2f all shipped, and the loop then grew a layer this plan did not
foresee: **matchup-first organization**. The recordings turned out to be
self-describing (char ids per frame), the roster and the `$40364D`
opponent+venue selector were fully mapped by headless probes, and the
architecture pivoted accordingly:

- per-matchup models (`--char`/`--opp` fit filters, slug-named dirs) and
  model SETS with automatic per-round matchup switching in the native runner;
- a `.rounds.jsonl` per-round matchup index written by the recorder, rendered
  as the 🥊 Matchup coverage grid (with force-matchup buttons that freeze the
  selector);
- gate v3: the composite gate gained the `$400006 == 0` term after probes
  showed v2 was open on the char-select screen.

Current working notes live in CLAUDE.md; addresses and probe evidence in
`library/asurabld/asurabld.md`. This file is kept as the historical plan of
record for waves 1–2f.
