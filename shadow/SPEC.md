# Shadow AI — Feature & Action-Space Specification (Wave 0A, rev. v2)

**Game:** Asura Blade — Sword of Dynasty (Fuuki FG-3, 1998)
**Core / frontend:** `fbalpha2012` under `rustretro`
**Purpose:** Behavioral cloning. A small model learns `game-state → the user's inputs`
and drives **P2** to play like the user, from demonstrations the user gives as **P1**.

This document is the contract that the in-frontend recorder (v2 shipped, Wave 2a) and
the Wave-2d training/deploy code implement against. It is design only — the only code
here is illustrative schema. Everything is grounded in the reverse-engineered ROM map
(`library/asurabld/asurabld.md`); addresses and offsets below are copied from that map,
not invented.

> **Revision v2 (2026-08-24).** Amended after the live-grounding sessions and external
> research (`shadow/PLAN.md` Wave 2c): corrected fighter-block model and per-round
> anchoring; health/meter/facing/char-id wired (no longer pending); the per-block hold
> accumulators are REMOVED as self-features (they track the opponent); the attack head
> is chord-aware (6 classes — the game's weapon toss and universal launcher are button
> chords); RETRO attack ids corrected to B/A/Y; gate v2 and the jsonl-v2 record schema;
> trainer commitments (§7) from shipped-system prior art (KI Shadow AI, Tekken ghosts).

---

## 0. Source-of-truth facts (from the ROM map) — v2 block model

All gameplay state lives in **Work RAM**, exposed by the Sek bus bridge as the window
`0x400000 + 0x10000` (see `asurabld.busmap.json`). Every field this spec reads is inside
that one window (input is captured at the frontend layer — see §5).

**Fighter data blocks** (0x0DB4 stride; slot→fighter assignment may vary per match/mode,
so the recorder captures both under neutral names and anchors per round — see §5):

| block | base |
|---|---|
| block1 | `$403798` |
| block2 | `$40454C` (= block1 + `0x0DB4`) |

**Confirmed per-block fields** (live write-tested and/or cheat-DB cross-confirmed —
see `asurabld.md` §Fighter data blocks):

| offset | field | type | notes |
|---|---|---|---|
| +0x00 | free-running frame timer | u16 | animation/recovery phase |
| +0x12 | walk / animation frame counter | u16 | ramps while moving, resets on state change |
| +0x50 | current action / command index | u16 | (+0x4C is a dup) |
| +0x54 | **screen X** (px) | u16 | round-start left ≈ 84, right ≈ 232 |
| +0x56 | **screen Y** (px) | u16 | ground = **216 (0xD8)**; dips through jumps |
| +0x61 | **facing** | u8 | 0 = facing left |
| +0x65 | weapon flag | u8 | 0 = armed |
| +0x177/+0x179 | **health pair** | u8×2 | max 0xEF; **regenerates ~1%/1.5 s standing neutral** — never assume monotone; pair semantics ("two stacked bars") under analysis |
| +0x17B | **super meter** | u8 | full = per-char max |
| +0x17F | per-char max meter | u8 | e.g. Yashaou 0x51, Footee 0x36 |
| +0x639 | **character id** | u8 | bosses 08/09 |

**REMOVED from the feature contract:** the `+0x28`/`+0x2A` "hold accumulators" — they
track the **opponent's** held direction (live-verified 2026-08-24), not the block's own
fighter. Hold features are now derived from recorded input-mask history (§1a).

**World-space:** P1 world X = `$4032EE` (= screen X + camera); opponent world X =
`$4027CE`. Screen-space per-block X/Y (above) is what the feature vector uses.

**Still pending:** projectile / extra-actor flags; the block-slot assignment rule across
modes (anchor per round instead of assuming — §5); 2P-VS layout verification (PLAN 2e).

**Related structures:** command-history rings at `$400FD8` (P1) + parallel P2 ring —
reserved for the macro-action layer (§3d). System control bytes (credits `$40655D`,
finish-round `$400000`, round timer `$40000A` BCD) per `asurabld.md` §system-control.

**Round / gate inputs** (used by the composite `controllable` gate in §5):

| addr | meaning |
|---|---|
| `$40646E` | round-over latch inside the fight loop |
| `$403678` | in-game abort (game over / continue) |
| `$402A32` | match end (nonzero = result) |
| `$40000A` | round timer, BCD seconds (valid BCD + nonzero ⇒ live round clock) |
| `$4041E7`/`$40470B` | cross-block combo counters (nonzero = other fighter in hitstun) |

---

## 1. Feature vector

The model consumes a **side-agnostic** vector (see §2) built from two actors, **me** and
**opp**. During recording, `me = P1` (human); at deploy, `me = P2` (bot). The vector below is
the *normalized* form the model sees; the record file (§5) stores the **raw** fields and the
normalization happens at train time.

All spatial values are `u16` screen pixels. Normalization constants (`X_SCALE`, `Y_SCALE`,
`TIMER_SCALE`, `ANIM_SCALE`, `CORNER_PX`) and `GROUND_Y` are **calibration parameters**,
stored alongside the model. Seeds: `GROUND_Y = 216 (0xD8)` (confirmed), `X_SCALE = 128`,
`Y_SCALE = 128`, `TIMER_SCALE = 256`, `ANIM_SCALE = 64`, `CORNER_PX = 24`. (`HOLD_SCALE`
is gone — v2 hold features are already [0,1] fractions.)

### 1a. The v2 vector (all sources shipped in recorder v2)

`me`/`opp` are resolved from block1/block2 via the per-round `p1_block` anchor (§5).
**Opponent-sourced features are fed 2–4 frames stale at train AND deploy time** (§4 —
the humanness rule; same staleness both places so train/deploy distributions match).

| # | feature | source | derivation | units after norm |
|---|---|---|---|---|
| 0 | `dist_x` | me/opp `x` | `s * (opp.x − me.x)` (§2); ≥0 = forward gap | / X_SCALE |
| 1 | `dy` | me/opp `y` | `opp.y − me.y` (down +) | / Y_SCALE |
| 2 | `me_airborne` | me `y` | `1 if (GROUND_Y − me.y) > 4 else 0` | {0,1} |
| 3 | `me_height` | me `y` | `clamp(GROUND_Y − me.y, 0, ∞)` | / Y_SCALE |
| 4 | `me_fwd_hold` | **input masks** | fraction of the last P frames my mask held forward (by `s`) | [0,1] |
| 5 | `me_back_hold` | **input masks** | same for back | [0,1] |
| 6 | `me_anim` | me `anim` | walk/animation counter | / ANIM_SCALE |
| 7 | `me_timer` | me `timer` | free-running frame timer (phase) | / TIMER_SCALE |
| 8 | `me_action` | me `action` | action/command index | **categorical** (see note) |
| 9 | `opp_airborne` | opp `y` | as #2 for opp | {0,1} |
| 10 | `opp_height` | opp `y` | as #3 for opp | / Y_SCALE |
| 11 | `opp_anim` | opp `anim` | as #6 for opp | / ANIM_SCALE |
| 12 | `opp_timer` | opp `timer` | as #7 for opp | / TIMER_SCALE |
| 13 | `opp_action` | opp `action` | as #8 for opp | **categorical** |
| 14 | `facing_sign` | me `facing` | the REAL facing bit → `s` (§2) | {−1,+1} |
| 15 | `me_health` | me `health` | `health / 0xEF` (regen ⇒ may rise) | [0,1] |
| 16 | `opp_health` | opp `health` | same | [0,1] |
| 17 | `health_lead` | derived | `me_health − opp_health` (aggression/turtle cue) | [−1,1] |
| 18 | `me_meter` | me `meter`,`meter_max` | `meter / meter_max` (per-char normalized) | [0,1] |
| 19 | `opp_meter` | opp | same | [0,1] |
| 20 | `me_hitstun` | gate `combo_on_me` | `1 if the opponent's combo counter on me != 0` | {0,1} |
| 21 | `opp_hitstun` | gate `combo_on_opp` | mirror | {0,1} |
| 22 | `me_corner` | me `x` | `1 if within CORNER_PX of a wall` (seed 24 px) | {0,1} |
| 23 | `opp_char` | opp `char_id` | opponent identity | **categorical** |

**Note (v2):** `opp_fwd_hold`/`opp_back_hold` from RAM are gone — the RAM accumulators
track the opponent (see §0) and the opponent's held direction is partially observable to
a human anyway; the opponent's *visible* state (position, anim, action) carries what a
player actually reacts to. `me`-hold features come from the model's own recorded input
masks, which are ground truth. #22/#17/#20-21 are the "intent-level" descriptors the
KI Shadow AI postmortem calls out as what makes retrieved behavior situationally right.

**`me_action` / `opp_action` (categorical):** the action-index id-space size is unknown and
likely large. Do **not** one-hot at high cardinality. Store the raw `u16` in the record; at
model build time map it through a **learned embedding** (recommended) or a coarse bucketing
of ids observed in the training set. This is a legitimate *observation of game state* (which
animation the fighter is in) — it is NOT the model's own previous output, so it does not
trigger the copycat trap (§4).

**Velocity** is intentionally absent as an explicit field (the position-vs-velocity split is
not yet reverse-engineered). It is recovered from **state stacking** (§4): stacking the last
K normalized snapshots lets the model infer dX/dt, dY/dt, closing speed, and jump vy. Do not
fabricate a velocity feature from a single frame.

Feature count v2: **21 scalars** (+ 3 categorical embeddings), before history stacking.

### 1b. Still pending (reserved placeholder slots — masked until mapped)

| name | eventual RAM source | why it matters |
|---|---|---|
| `proj_present`, `proj_dx`, `proj_dy` | projectile / extra-actor flags (pending) | reacting to fireballs / weapon tosses |
| `me_weapon` | weapon flag +0x65 (mapped, semantics per-char) | disarmed movesets differ; wire once toss demos exist |

Placeholders are carried as fixed reserved indices so adding them later does not renumber
existing features; until mapped they are masked (constant 0 + a `*_valid=0` companion bit).

---

## 2. Side-agnostic normalization (the critical transform)

**Goal:** every feature is expressed as *me vs opponent* and *forward vs backward*, never
*left/right* or *P1/P2*. This is what lets P1 (human) demonstrations drive a P2-side bot: the
model never sees which physical side it is on.

**Facing / forward sign — v2: use the real facing byte.** Each block's `facing` field
(+0x61: 0 = facing left, 1 = facing right; flips as fighters cross) gives the sign
directly:

```
s = +1 if me.facing == 1 else −1     # forward = the way my character faces
```

This disambiguates crossups exactly (the game updates the byte on its own crossup
logic). Keep the old position-derived rule ONLY as a validation cross-check during
dataset building: flag frames where `sign(opp.x − me.x)` disagrees with `s` outside a
crossup window — persistent disagreement means the block anchor (§5) is wrong for that
round.

**Forward-relative spatial features.**
```
dist_x = s * (opp.X − me.X)   = |opp.X − me.X|   # ≥ 0, always "opponent ahead"
dy     = opp.Y − me.Y                             # vertical needs no mirror (up/down are absolute)
```

**Forward-relative movement holds — v2: from input masks.** The RAM accumulators are
opponent-linked (§0) and are NOT used. `me`'s holds come from the recorded 12-bit masks:
over the last P frames, `me_fwd_hold` = fraction of frames whose mask held Right when
`s>0` (Left when `s<0`); `me_back_hold` mirrors. Side-agnostic by construction since the
selection is by `s`.

**Heights** are already absolute-vertical (`GROUND_Y − Y`), no mirroring.

**Two directions of use (why this closes the loop):**
- *Train:* build samples with `me = P1`, `opp = P2`, compute `s` from their X's, and encode
  the label (§3) in the same forward-relative frame. (You may also mine `me = P2` samples
  from human-vs-human recordings; the transform is identical.)
- *Deploy:* the bot is P2. Compute features with `me = P2`, `opp = P1`, and the deploy-time
  `s_deploy` from *their* X's. The model outputs forward-relative intents; §3's inverse map
  turns them back into absolute P2 joypad bits using `s_deploy`. Same weights, either side.

---

## 3. Action space

### 3a. Intent set (two factored categorical heads)

Fighting inputs are *direction + button held simultaneously*; a single flat multiclass over
their product explodes and starves the tails. Use **two independent heads**:

**Move head — 9 classes** (forward-relative):
`0 Neutral · 1 Forward · 2 Back · 3 Up(jump) · 4 Down(crouch) · 5 Up-Forward · 6 Up-Back ·
7 Down-Forward · 8 Down-Back`.
Block is **not** a button — it is holding **Back** (class 2 / 6 / 8), so it falls out of the
move head for free.

**Attack head — 6 classes, chord-aware (v2).** Asura Blade has exactly **3 attack
buttons** (Light/Medium/Heavy slash), and its two universal system actions are **button
chords**: the launcher/wall-bounce "Bash Attack" = any two attack buttons, weapon toss =
all three (FAQ-verified; see `asurabld.md` §Input mapping). Chords are first-class
demonstrated actions, so the head is:
`0 None · 1 Light · 2 Medium · 3 Heavy · 4 Launcher (any-2) · 5 Toss (all-3)`.
(Multi-frame special-move MOTIONS remain out of scope for v1 — §3d.)

Label per decision = `(move_class, attack_class)`. Two softmax heads, sampled independently.

### 3b. Why classification + sampling, not 12-bit regression

Per-frame regression of 12 independent joypad bits (12 sigmoids) is the wrong objective:

- **Illegal / incoherent combos.** Independent bits happily emit Left+Right or
  Up+Down+all-buttons. The factored classes are legal by construction.
- **Mean collapse.** Regressing to demonstrated bits under MSE/BCE averages a bimodal human
  ("sometimes jump, sometimes block") into a mushy in-between that does neither.
- **No stochasticity.** Behavioral cloning must *reproduce the user's variability and
  habits*. Softmax + **temperature sampling** replays the demonstrated distribution
  (including mistakes, §6); a regressed point estimate cannot.
- **Right loss.** Cross-entropy against the empirical action distribution is exactly the
  BC objective; it also handles class imbalance (Neutral dominates) via weighting.

### 3c. RETRO_DEVICE_ID joypad mapping

RETRO id order (given): `0=B 1=Y 2=Select 3=Start 4=Up 5=Down 6=Left 7=Right 8=A 9=X 10=L 11=R`.

**Direction bits** (built from the move head, using deploy-side `s_deploy`):

| move class | Up(4) | Down(5) | Left(6) | Right(7) |
|---|---|---|---|---|
| Neutral | | | | |
| Forward | | | `s<0` | `s>0` |
| Back | | | `s>0` | `s<0` |
| Up | ● | | | |
| Down | | ● | | |
| Up-Forward | ● | | `s<0` | `s>0` |
| Up-Back | ● | | `s>0` | `s<0` |
| Down-Forward | | ● | `s<0` | `s>0` |
| Down-Back | | ● | `s>0` | `s<0` |

**Attack bits — v2, source-verified** (fbalpha2012 driver source + live injection test,
2026-08-24): the game polls exactly `B(0) = Light`, `A(8) = Medium`, `Y(1) = Heavy`;
`X(9)`/`L(10)`/`R(11)` are **never polled**. Class → bits:
`Light → {B}` · `Medium → {A}` · `Heavy → {Y}` · `Launcher → {B,A}` · `Toss → {B,A,Y}`.
`Select(2)`/`Start(3)` are not gameplay outputs and are never set by the bot.

**Build the 12-bit word:** `OR` the move head's direction bits with the attack head's single
button bit. Output is a 12-bit mask fed to P2 via the frontend's input injection.

### 3d. Label extraction (record → training label) and out-of-scope

At train time, invert the map: from a recorded actor's raw 12-bit input **and that frame's
`s`**, recover `(move_class, attack_class)` in the forward-relative frame. Left/Right → F/B
by `s`. Attack class from the pressed attack-bit set (B/A/Y only):
`{} → None` · one bit → its class · **any two bits → Launcher** · **all three → Toss**.
(Collapse the three 2-bit combinations into one Launcher class — the game treats any-2
identically; EX inputs also press two buttons but require a motion, which v1 doesn't
model, so they label as Launcher and are cloned only as such.)

**Out of scope for v1:** multi-frame special-move motions (QCF+P etc.) whose input windows
are faster than the 8 Hz decision rate. At 8 Hz the bot clones neutral game, movement,
blocking, normals, launchers, and tosses. **The committed path for specials (v2+, per the
KI Shadow AI precedent): a macro-action layer** — mine the user's recurring input strings
offline (the command ring `$400FD8` is the game's own detector buffer), store them as
atomic replayable sequences with exact frame timing, and let a head choose *which string
to start*; the per-tick heads never reconstruct motions. Do NOT attempt a higher-rate
motion sub-head first.

---

## 4. Decision rate & timing

**Decide at ~8 Hz.** The game runs ~60 fps, so the decision period is
`P = round(fps / 8) ≈ 8 frames` (7.5 Hz). Rationale:
- Matches human visual reaction (~200 ms); a bot that re-decides every frame is
  superhuman and reads as robotic.
- **Hides inference latency:** one 8 Hz slot is ~125 ms, comfortably longer than a small
  model's forward pass, so decisions never stall the frame loop.
- The chosen `(move, attack)` intent is **held** across the P frames until the next decision
  (matching how humans hold inputs), then re-sampled.

**Stale opponent observations (v2 humanness rule).** Every system that shipped a
convincing human-like fighter adds deliberate observation delay (FightingICE mandates 15
frames; slippi-ai runs 18+; KI shadows "guess like humans"; human reaction ≈ 12–19 frames
at 60 fps). All **opponent-sourced** features (#1, #9–13, #16, #19, #21 and `dist_x`'s
opp term) are read **2–4 frames stale** — and identically stale at train and deploy time,
so the learned mapping matches what the bot will see. Self-features stay current (you
know your own hands). Combined with the 8 Hz cadence this bounds effective reaction at
~10–12 frames: human, not robotic.

**Recorder runs at full 60 Hz** (every frame — §5). Decision granularity is a *train/deploy*
concern, decoupled from capture:
- *Train:* decimate to one labeled example per `P` frames. The label for a decision window is
  the input the actor **held most of the window** (mode of the P raw inputs), so a brief
  1-frame tap between slots isn't lost as label but also doesn't create a phantom decision.
  Keeping all raw frames means the decision rate can be re-tuned later without re-recording.
- *Deploy:* run the model once per `P` frames, hold the output between decisions.

**Copycat trap — do NOT feed the bot its own previous action label as a feature.** A model
that sees "what I just pressed" learns the trivial identity `next = last` and freezes into
repetition, decoupled from the opponent. Instead give temporal context by **stacking the last
K decision-step STATE snapshots** (recommend `K = 4`, i.e. ~0.5 s of history) of the §1
normalized vector. This supplies velocity/closing/jump-arc information (which no single frame
carries, §1) without leaking the policy's own outputs. Note the distinction from `me_action`
(§1a): that is the *game's* animation-state read from RAM (a real observation), not the
model's emitted intent.

**Frame-timing features.** Encode the instantaneous `+0x00` free-running timer (`me_timer`)
and the `+0x12` animation counter (`me_anim`) as features so the model knows *where in an
animation/recovery it is* — the difference between "can act now" and "still in recovery" —
which is exactly the information a per-frame human reads from character pose.

---

## 5. Record file format

**Format: JSONL** — one JSON object per line, one line per emulated frame (60 Hz).

*Why JSONL over CSV:* the schema will grow (pending fields in §1b become real columns; nested
per-actor blocks; optional metadata). JSONL is self-describing, appends in a streaming loop,
tolerates added/optional keys without breaking older readers, and represents `null`
placeholders cleanly. CSV's fixed column order makes every schema change a breaking migration.

**Store RAW fields, normalize at train time.** The file holds the untransformed struct words
and raw input bits — never normalized/side-agnostic values. This decouples the on-disk record
from the evolving feature schema (§1/§2): re-tuning `GROUND_Y`, scales, the `s` definition, or
adding pending fields never invalidates existing recordings.

**Per-row schema (jsonl-v2, as shipped in `src/record.rs`):**

```jsonc
{
  "frame": 918273,            // monotonic recorder frame index (u64)
  "round_id": 3,              // increments on each gate false→true edge
  "controllable": true,       // composite gate — see below
  "p1_block": 1,              // anchor: which block is P1 this round (1|2|null)
  "block1": {                 // raw fields, base $403798 — NEUTRAL slot name
    "x": 122, "y": 216, "facing": 1, "weapon": 0,
    "health": 239, "health2": 239,      // +0x177/+0x179 pair
    "meter": 64, "meter_max": 81,       // +0x17B / +0x17F
    "char_id": 0, "wins": 0,
    "timer": 3541, "anim": 12, "action": 9,
    "opp_right_hold": 0, "opp_left_hold": 0  // OPPONENT-linked (§0) — analysis only
  },
  "block2": { /* same shape, base $40454C */ },
  "gate": {                   // raw gate inputs, so training can re-derive
    "round_over": 0, "abort": 0, "match_end": 0,
    "timer_bcd": 133, "demo_flag": 1,
    "combo_on_b1": 0, "combo_on_b2": 0, "credits": 8
  },
  "p1_input": 128,            // 12-bit RETRO joypad mask this frame (u16)
  "p2_input": 0
}
```

**Field capture notes:**
- Actor fields: read directly from the Work RAM window (`0x400000+0x10000`) at each actor
  base. All §1 offsets are inside it.
- **Inputs:** the gamepad word `$810000` is *not* in a declared bus window, so do not rely on
  the bridge for it. The recorder lives in the frontend and is the component that
  reads/injects the libretro joypad, so capture `p1_input`/`p2_input` **at the frontend input
  layer** (the authoritative 12-bit RETRO masks). (Alternative if that's inconvenient: add a
  `$810000` word window to the busmap and split by nibble — P1 low, P2 high — per the
  coin/start service at `$023BB2`.) Human-vs-CPU demos yield real `p1_input` and `p2_input=0`
  (P2 is AI-driven internally, not via the pad); to demonstrate P2-side habits directly,
  record human-vs-human and both masks are populated.
- **`controllable` gate (v2, live-validated):** composite — hop flags clear AND both
  blocks' health in `1..=0xEF` AND the round timer reads as nonzero valid BCD. False at
  boot/title/menus/continue; opens at each round start; **closes at the KO** (post-KO
  frames excluded). Two train-time filters on top: (1) **demo rounds** pass the gate by
  design — drop any `controllable` round whose `p1_input` mass is zero (an attract demo
  by definition; char-id pairs corroborate); (2) block-anchor sanity via the §2 facing
  cross-check. `$4065D8` is a fight-progress latch, NOT a demo flag — do not gate on it.
- **Anchor:** `p1_block` is resolved at each round-start edge (smaller `x` = left = P1)
  and sticky for the round. The trainer maps `me/opp` from it; never assume slot order.

Recommended layout: one JSONL file per recording session under `shadow/recordings/`, plus a
sidecar `*.meta.json` (core version, game, character picks, fps, calibration constants used).

---

## 6. Scope notes

- **Per-character models.** One model per character; **start with Footee or Yashaou.** Action-
  index semantics, position ranges, animation strides (e.g. Footee's idle at stride 0x19),
  and move properties differ per character, so a shared model would blur them. Character
  identity (own and opponent) is itself a **pending** RAM field (§1b) — record it as `null`
  now and infer/label from the char-select scene until mapped.
- **Behavioral cloning clones habits, mistakes and all.** There is no reward signal and no
  self-play — the model reproduces *this user's* tendencies: their favored ranges, their
  reactions, their bad habits and dropped inputs. Temperature sampling (§3b) is what makes
  those habits show up as behavior rather than being averaged away. If the user whiffs a move
  on wakeup, a faithful clone sometimes whiffs it too. This is the intended product, not a
  bug; superhuman/optimal play is explicitly a non-goal for these waves.

---

## 7. Trainer commitments (v2) — from shipped-system prior art

Grounded in the KI Shadow AI postmortem, Tekken 8 Ghost reception, and the
imitation-learning literature (sources in the Wave-2c research reports):

1. **kNN baseline first.** KI shipped instance retrieval at exactly our data scale, and
   retrieval cannot hallucinate. Build the case-store baseline (normalized state → the
   user's action, retrieved by similarity over §1's vector) before any parametric model;
   the MLP must beat it on the §7.4 eval to replace it.
2. **Support constraint — "everything it does, you did."** The sampler may only emit an
   action with nonzero empirical support in similar states (state-bucket mask, or
   reject-and-resample against the kNN store). A novel-but-plausible action breaks the
   "that's me" illusion faster than repetition does. This is the property both KI and
   Tekken 8 shipped and marketed.
3. **Defense is the known failure mode — weight for it.** Every shipped clone drew the
   same complaint: copies offense, never blocks. Defensive transitions (block-then-punish,
   anti-air, corner escape) are decision-sparse; oversample/upweight them, and evaluate
   block/duck/movement rates PER SITUATION bucket, not globally.
4. **Evaluation stack:** (a) held-out next-action accuracy per situation bucket
   (neutral / offense / defense / oki / corner); (b) conditional action-frequency match
   against the user's empirical distribution per bucket; (c) the A/B clip test — show the
   user unlabeled clone-vs-self clips (note: players misjudge their own habits; pair with
   (b), don't rely on self-report alone).
5. **"Improves over time" stays boring.** Retrain from scratch each session on a sliding
   window of recent recordings (player behavior drifts as they improve); fixed
   architecture and temperature across sessions so progress reads as "it learned my new
   mixup", never a personality change. kNN variant: append cases, cap by recency
   (KI: 40 matches per matchup).
6. **Coverage report as a user-facing feature.** After each retrain, list situation
   buckets with fewer than N examples ("anti-air: 3, corner defense: 1") — it tells the
   user what to demonstrate next in training mode, turning the data gap into a drill list.
