# Shadow AI — Feature & Action-Space Specification (Wave 0A)

**Game:** Asura Blade — Sword of Dynasty (Fuuki FG-3, 1998)
**Core / frontend:** `fbalpha2012` under `rustretro` (branch `feature/execution-map`)
**Purpose:** Behavioral cloning. A small model learns `game-state → the user's inputs`
and drives **P2** to play like the user, from demonstrations the user gives as **P1**.

This document is the contract that the Wave-1 in-frontend recorder and the Wave-2
training/deploy code both implement against. It is design only — the only code here is
illustrative schema. Everything is grounded in the reverse-engineered ROM map
(`library/asurabld/asurabld.md`, `asurabld.busmap.json`); addresses and offsets below are
copied from that map, not invented.

---

## 0. Source-of-truth facts (from the ROM map)

All gameplay state lives in **Work RAM**, exposed by the Sek bus bridge as the window
`0x400000 + 0x10000` (see `asurabld.busmap.json`). Every field this spec reads is inside
that one window, so the recorder needs no new bus windows for state (input is a separate
question — see §5).

**Fighter actor structs** (identical layout, one shared controller drives both):

| actor | base | stride |
|---|---|---|
| P1 (the human demonstrator) | `$40454C` | — |
| P2 (the CPU / the bot we deploy) | `$405300` | P1 + `0x0DB4` |

**Confirmed field offsets** (from struct base; both actors identical):

| offset | field | type | notes |
|---|---|---|---|
| +0x00 | free-running frame timer | u16 | counts down every frame, all states (animation/recovery phase) |
| +0x12 | walk / animation frame counter | u16 | ramps while moving, resets on state change |
| +0x14 | dup of +0x12 | u16 | |
| +0x28 | right-movement hold accumulator | u16 | ramps only while holding right (absolute) |
| +0x2A | left-movement hold accumulator | u16 | ramps only while holding left (absolute) |
| +0x4C | current command / action index | u16 | decoded pad/action each frame |
| +0x50 | dup of +0x4C | u16 | |
| +0x54 | **X position** (screen px) | u16 | `+= right`; observed ~0x78→0xB6 walking into a wall |
| +0x56 | **Y position** (screen px) | u16 | `+= down`; ground ≈ **0xD8**; dips through a jump arc |
| +0x5A | secondary X ref | u16 | tracks +0x54 in lockstep (offset ~+0x60) |
| +0x5C | secondary Y ref | u16 | tracks +0x56 in lockstep |

**Pending (not yet mapped — reserved as placeholder inputs):** facing bit, health, meter,
projectile / extra-actor flags, character identity, absolute-vs-camera X split, velocity as
a distinct field (currently derived, see §1/§4).

**Related structures:** P1 command-history ring at `$400FD8` (special-move detector's input
buffer; a parallel P2 ring exists). Gamepad hardware word `$810000` (P1 low byte, P2 high
byte) — **note: `$810000` is NOT in a declared bus window**, so it is not readable through
the bridge as configured (see §5 for how the recorder captures input instead).

**Round / controllable hop-flags** (Work RAM, from the execution map — used to gate the
`controllable` flag in §5):

| addr | meaning |
|---|---|
| `$40646E` | round-over latch inside the fight loop |
| `$403678` | in-game abort (game over / continue) |
| `$402A32` | match end (nonzero = result) |
| `$40636C/E` | per-side round-end pulses |

---

## 1. Feature vector

The model consumes a **side-agnostic** vector (see §2) built from two actors, **me** and
**opp**. During recording, `me = P1` (human); at deploy, `me = P2` (bot). The vector below is
the *normalized* form the model sees; the record file (§5) stores the **raw** fields and the
normalization happens at train time.

All spatial values are `u16` screen pixels. Normalization constants (`X_SCALE`, `Y_SCALE`,
`HOLD_SCALE`, `TIMER_SCALE`, `ANIM_SCALE`) and `GROUND_Y` are **calibration parameters** —
seed values are given, to be finalized by Wave-1 calibration and stored alongside the model.
Seeds: `GROUND_Y = 0xD8 (216)`, `X_SCALE = 128`, `Y_SCALE = 128`, `HOLD_SCALE = 64`,
`TIMER_SCALE = 256`, `ANIM_SCALE = 64`.

### 1a. Available today

| # | feature | RAM source | derivation | units after norm |
|---|---|---|---|---|
| 0 | `dist_x` | me/opp +0x54 | `s * (opp.X − me.X)` (§2); ≥0 = forward gap | / X_SCALE |
| 1 | `dy` | me/opp +0x56 | `opp.Y − me.Y` (down +) | / Y_SCALE |
| 2 | `me_airborne` | me +0x56 | `1 if (GROUND_Y − me.Y) > 4 else 0` | {0,1} |
| 3 | `me_height` | me +0x56 | `clamp(GROUND_Y − me.Y, 0, ∞)` | / Y_SCALE |
| 4 | `me_fwd_hold` | me +0x28/+0x2A | `s>0 ? right_hold : left_hold` (§2) | / HOLD_SCALE |
| 5 | `me_back_hold` | me +0x28/+0x2A | the other accumulator | / HOLD_SCALE |
| 6 | `me_anim` | me +0x12 | walk/animation counter | / ANIM_SCALE |
| 7 | `me_timer` | me +0x00 | free-running frame timer (phase) | / TIMER_SCALE |
| 8 | `me_action` | me +0x50 | current action/command index | **categorical** (see note) |
| 9 | `opp_airborne` | opp +0x56 | as #2 for opp | {0,1} |
| 10 | `opp_height` | opp +0x56 | as #3 for opp | / Y_SCALE |
| 11 | `opp_fwd_hold` | opp +0x28/+0x2A | opp's forward = toward me (`−s`) | / HOLD_SCALE |
| 12 | `opp_back_hold` | opp +0x28/+0x2A | the other accumulator | / HOLD_SCALE |
| 13 | `opp_anim` | opp +0x12 | as #6 for opp | / ANIM_SCALE |
| 14 | `opp_timer` | opp +0x00 | as #7 for opp | / TIMER_SCALE |
| 15 | `opp_action` | opp +0x50 | as #8 for opp | **categorical** |
| 16 | `facing_sign` | derived | `s` from §2 (proxy until real facing bit lands) | {−1,+1} |

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

Feature count today: **17 scalars** (+ 2 categorical embeddings), before history stacking.

### 1b. Pending (reserved placeholder slots — emit `null` in records now, wire in when mapped)

| name | eventual RAM source | why it matters |
|---|---|---|
| `me_facing`, `opp_facing` | facing bit (pending) | replaces the `s` proxy (#16); disambiguates crossups |
| `me_health`, `opp_health` | health (pending) | who's ahead → aggression vs turtling |
| `me_meter`, `opp_meter` | meter (pending) | super availability |
| `proj_present`, `proj_dx`, `proj_dy` | projectile / extra-actor flags (pending) | reacting to fireballs / weapon tosses |
| `char_id` (me), `char_id` (opp) | char-select result (pending) | model is per-char (§6) but opp id is a useful input |

Placeholders are carried in the vector as fixed reserved indices so that adding them later
does not renumber existing features. Until mapped, they are masked (constant 0 + a
`*_valid=0` companion bit) so the model learns to ignore them.

---

## 2. Side-agnostic normalization (the critical transform)

**Goal:** every feature is expressed as *me vs opponent* and *forward vs backward*, never
*left/right* or *P1/P2*. This is what lets P1 (human) demonstrations drive a P2-side bot: the
model never sees which physical side it is on.

**Facing / forward sign.** Fighters almost always face the opponent, so until the real facing
bit is mapped we use position:

```
raw = opp.X − me.X
s   = +1 if raw >  EPS       # opponent is to my right  → forward = +X (screen right)
      −1 if raw < −EPS       # opponent is to my left   → forward = −X (screen left)
      s_prev otherwise       # |raw| ≤ EPS: hysteresis, hold last sign (crossover ambiguity)
```
`EPS ≈ 4 px`. When the true facing bit lands (§1b), replace `s` with it directly.

**Forward-relative spatial features.**
```
dist_x = s * (opp.X − me.X)   = |opp.X − me.X|   # ≥ 0, always "opponent ahead"
dy     = opp.Y − me.Y                             # vertical needs no mirror (up/down are absolute)
```

**Forward-relative movement holds.** The accumulators are absolute (right = +0x28,
left = +0x2A). Selecting by `s` makes them side-agnostic:
```
me_fwd_hold  = (s > 0) ? me.right_hold  : me.left_hold
me_back_hold = (s > 0) ? me.left_hold   : me.right_hold
opp_fwd_hold = (s > 0) ? opp.left_hold  : opp.right_hold   # opp's forward is −s (toward me)
opp_back_hold= (s > 0) ? opp.right_hold : opp.left_hold
```

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

**Attack head — 5 classes** (Asura Blade = 3 attack buttons + weapon toss):
`0 None · 1 Attack-A (weak) · 2 Attack-B (medium) · 3 Attack-C (strong) · 4 Weapon-toss`.
(One button per decision keeps the head small; simultaneous multi-button and multi-frame
special-move motions are out of scope for v1 — see §3d.)

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

**Attack bits** — RETRO id per in-game button is a **calibration parameter**, confirmed once
via `press_buttons` (per the fighting-game RE methodology: inject one button, watch the actor
react). Seed mapping (fbalpha fightstick convention, to be verified):
`Attack-A → Y(1)`, `Attack-B → X(9)`, `Attack-C → B(0)`, `Weapon-toss → A(8)`.
`Select(2)`/`Start(3)`/`L(10)`/`R(11)` are not gameplay outputs and are never set by the bot.

**Build the 12-bit word:** `OR` the move head's direction bits with the attack head's single
button bit. Output is a 12-bit mask fed to P2 via the frontend's input injection.

### 3d. Label extraction (record → training label) and out-of-scope

At train time, invert the map: from a recorded actor's raw 12-bit input **and that frame's
`s`**, recover `(move_class, attack_class)` in the forward-relative frame. Left/Right → F/B by
`s`; the first pressed attack bit → attack class; no attack bit → class 0.

**Out of scope for v1 (document, don't attempt):** multi-frame special-move motions (QCF+P
etc.) whose input windows are faster than the 8 Hz decision rate. At 8 Hz the bot clones
neutral game, walks, jumps, crouches, blocks, and normal/weapon attacks. Specials will need
either a higher-rate motion sub-head or a macro/detector layer keyed off the command ring
(`$400FD8`) — deferred to a later wave.

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

**Per-row schema:**

```jsonc
{
  "frame": 918273,            // monotonic recorder frame index (u64)
  "round_id": 3,              // increments each round; ties rows to a bout
  "controllable": true,       // gameplay input accepted this frame — see gate below
  "p1": {                     // raw actor fields, base $40454C
    "x": 122, "y": 216,       // +0x54, +0x56
    "action": 9, "action2": 9,// +0x50, +0x4C
    "timer": 3541,            // +0x00
    "anim": 12, "anim2": 12,  // +0x12, +0x14
    "right_hold": 40, "left_hold": 0, // +0x28, +0x2A
    "x2": 218, "y2": 216,     // +0x5A, +0x5C
    "health": null, "meter": null, "facing": null // pending (§1b)
  },
  "p2": { /* same shape, base $405300 (= P1 + 0x0DB4) */ },
  "p1_input": 128,            // 12-bit RETRO joypad mask this frame (u16)
  "p2_input": 0,              // "
  "char": { "p1": null, "p2": null } // pending char id (§6); fill when mapped
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
- **`controllable` gate:** true only during live fighting — derive from the execution-map hop
  flags: round-intro finished AND `$40646E` (round-over) not latched AND `$403678` (abort) == 0
  AND `$402A32` (match end) == 0. Training filters to `controllable == true` rows so menus,
  intros, KO freezes, and continue screens never become examples.

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
```
