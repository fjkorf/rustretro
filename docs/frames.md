# Frames — anatomy of a hit, and the frame lab

Normative contract for measured frame data. It exists because "is this move
safe?" is the first question in this project that is a MEASUREMENT rather
than an address, and because every published fighting-game frame table
disagrees with every other one about what the numbers mean.

Companion docs: `signal-hunt.md` (how fields get found), `game-profiles.md`
(where addresses live), `shadow/MACRO_ACTIONS.md` (how moves are named and
encoded). This doc owns: the vocabulary, the measurement protocol, the
storage schema, and the honesty rules.

> **Revision note.** The first draft of this contract named MK2 arcade
> `hit_counter 0xD3FE` as the contact anchor and the mapped `x` field as the
> actionability observable. Both were already disproven in `library/mk2/mk2.md`
> before this doc was written — `0xD3FE` is a P1-victim-only counter
> (mk2.md "Contact-signal correction") and `p1_x`/`p2_x` live in a dynamic
> `0x42`-stride object pool whose slot is not stable across boots
> (mk2.md "Toolkit friction"). Two reviewers caught it. The corrected forms
> are §4.1 and §4.2. Recorded here rather than silently edited, because
> "the contract asserted a signal the evidence doc had already retracted" is
> exactly the failure mode §7 exists to prevent.

## 1. Anatomy — the shared vocabulary

```
              0    5    10   15   20   25   30
              ├────┼────┼────┼────┼────┼────┤   (frames, 1 char = 1 core frame)
P1 (attacker) ░░░░░░░░░▓▓▓▓▒▒▒▒▒▒▒▒▒▒▒▒▒▒◆
              ↑press   ↑contact           ↑ actionable f27
P2 (defender)          ██████████████◆
                       ↑stun starts     ↑ actionable f23
                                     ├───┤
                                     P2 free 4f earlier → P1 is −4 on block

  ░ startup   committed, hitbox not yet out
  ▓ active    hitbox out; contact = first active frame that overlaps
  ▒ recovery  hitbox gone, still locked — the entire punish window
  █ stun      blockstun or hitstun
  ◆ actionable — the ONE thing this lab actually measures (§4)
```

**Advantage** is not a property of a move. It is a difference between two
clocks started at the same instant:

```
advantage = (frame the defender becomes actionable)
          − (frame the attacker becomes actionable)
```

**Punishable** is advantage compared against what the opponent can reach:

```
punishable ⟺ advantage ≤ −(opponent's fastest first_active_frame)
             AND the post-contact gap is inside that move's connect range
```

The range clause is not a footnote. A move that is −8 but shoves the
defender past their own connect range is safe in practice. Connect ranges
are part of this table (§6), not a separate concern.

### 1.1 Outcomes

Every attack resolves to exactly one of these, and only the first group
produces an advantage number:

| Outcome | Clock | Advantage meaningful? |
|---|---|---|
| whiff | attacker's recovery only | no — nobody is stunned |
| block | blockstun | **yes** |
| hit (grounded) | hitstun | **yes** |
| counter-hit | extended hitstun | yes, separate row |
| trade | both stunned | difference of two stuns |
| airborne hit / juggle | gravity | no — the juggle owns the timing |
| knockdown | wakeup | no — measure the WAKEUP WINDOW instead |
| KO | none | n/a |

Conflating knockdown with hit advantage is the classic error. A sweep does
not have an "on hit" number; it has a getup window, measured by the same
protocol but stored in a different column.

### 1.2 Hitstop

On contact both fighters freeze for N frames. Because it applies to both
lanes equally it CANCELS OUT of advantage — but it does not cancel out of
measurement: injected input is eaten during it (this is what the training
dummy's fitted `PUNISH_DELAY` was compensating for). Hitstop is measured and
stored as its own column, never folded into startup or recovery.

## 2. Conventions — decided once, here

Every number in the database obeys these. They are stated because the two
biggest published-frame-data failures in the genre are convention failures,
not measurement failures.

1. **`first_active_frame` (FAF) is canonical.** The stored integer is the
   frame number, 1-indexed from the input frame, on which contact can first
   occur. NRS counts startup EXCLUDING the first active frame, so an NRS
   "7-frame" move connects on frame 8 — their own players miscalculate
   punishes over this. FAF is unambiguous; both notations are one
   subtraction away. Renderers MUST label which they show
   (`Startup 8 (FAF)`), and MUST NOT print a bare startup number.
   FAF's operational definition is in §4.4 — it is NOT a by-product of the
   actionability probe, and claiming it is was a defect in the first draft.
2. **Logic frames, not video frames.** One row = one core frame = one
   `retro_run`. Numbers come from RAM, never from counting a recording.
3. **Sprite lag is a measured calibration, not an assumption.** MK's sprites
   are reported to lag the logic by one tick, which would make every
   video-counted community number differ from ours by a constant. Store it
   as `sprite_lag_frames` per port with its own evidence; do NOT silently
   apply an offset to reconcile with an external table.
4. **Wall-clock is never a unit.** No sleeps, no milliseconds. Earlier
   sessions produced "~200–370 ms" windows by sampling on sleeps; those
   numbers are unusable here. Everything steps.
5. **Absent means absent.** A quantity we could not measure is NULL, never
   0 — the RECORDER_V3 law, restated because a zeroed startup reads as an
   instant move.
6. **Hit-vs-block is a property of the RIG, not an inference.** The lab
   drives both ports, so it KNOWS whether the defender was holding guard.
   Inferring block from a health delta is only necessary for live or
   recorded play, and on MK2 arcade it is unreliable in both directions
   (blocked contact chips −3/−6, so "zero damage" means WHIFF, not block —
   mk2.md "Hitstun / blockstun observables"). Rows record the rig's guard
   state; the inferred value, where computed, is a separate column.

## 3. Preconditions — every one of these is a precondition, not a nicety

A measurement run that skips any of these is void.

1. **Training enforcement OFF.** `run_lua("training.set_enabled(false)")`.
   Not because the dummy stomps the probe — it does not; `training.rs`
   writes `injected_input2` while `held_input2` is OR'd in afterwards — but
   because the health-refill enforcement rewrites `0xBCA0`/`0xBC88`, which
   are simultaneously the contact anchor and the damage reading. Refill and
   anchor are the same bytes.
2. **Shadow runner disabled.** Any model driving a port invalidates the run.
3. **`hold_buttons`/`release_buttons` only. `press_buttons` is BANNED in the
   lab.** Its countdown decrements on every GUI frame including while
   paused, so a chord can evaporate between the press and the step
   (`src/mcp/server.rs`, and mk2.md's toolkit-friction note that it needs
   real wall-clock to elapse after the call returns). A protocol built on it
   is wall-clock-dependent, which §2.4 forbids.
4. **Arena liveness re-verified after EVERY `load_state`**, not once at
   capture. The sidecar's `inputs_live` is a save-time assertion; the same
   object-pool instability that breaks `x` can invalidate it later.
5. **Every `step` confirmed to have LANDED.** `step` is fire-and-forget:
   30 rapid calls were measured landing **1** frame. Poll `get_state`'s
   `frame_count` until it advances before issuing the next step. A protocol
   that skips this measures a frame count that never happened, and it
   produces a "the input did nothing" false negative that looks exactly like
   a real result (it briefly convinced one agent that held input cannot
   reach the core during stepping — it can: +72 and +63 units over 30
   confirmed frames, control 0).
6. **Let the frame FINISH before changing input.** Confirming a `step`
   (frame_count moved) does NOT prove the emulated frame completed. Roughly
   1 run in 50, a `hold_buttons` issued immediately after the confirmation
   landed one frame EARLY — a single spurious TRUE below the real boundary,
   never reproducing on re-run, silently moving the answer by several frames.
   A/B over 14 identical pairs: 1 flake with no settle, 0 with an 8 ms
   settle; before it one sweep failed its repeat check 5/5, after it all four
   sweeps passed first try. Settle, or make `step` synchronous.
7. **Every `load_state` confirmed to have LANDED.** Loads do not drain while
   paused. Resume, load, verify against a known field, then pause. Skipping
   this silently measures the PREVIOUS state.
8. **Zero-point calibration current** (§3.1) for this core build and ROM.

Note on §2.4: waiting in wall-clock for a step or load to LAND is transport
bookkeeping, not measurement. The ban is on wall-clock as a UNIT — no
duration in this database is ever expressed in milliseconds. Polling until a
frame lands and then counting frames is compliant; sleeping 500 ms and
calling it 30 frames is not.

### 3.1 Zero-point calibration

The probe measures "when did injected input take effect," which is only a
frame count if we know the injection latency. Per port, before any move data
is collected:

1. Neutral, both fighters idle, nothing driving either port.
2. Hold a walk direction at a known frame `F`.
3. Step; record the first frame at which the fighter's observable (§4.2)
   diverges from a no-input control run.
4. `input_latency_frames = that frame − F`. Repeat ≥5 times; it MUST be
   constant. If it is not, STOP — the probe is not sound on this port and
   nothing downstream can be trusted.

**Calibrate the probe's OWN input shape, not a bare neutral walk.** Sizing
from the wrong shape produces a confident silent wrong answer. The guarded
defender probe releases a held Block AND walks on the same frame, and MK2's
block stance does not drop when the button does: neutral calibration says
latency 2 for pointer-`x`; the real probe latency is **11**. Sized from the
neutral number, the on-block defender sweep reported NEVER ACTIONABLE across
all 46 candidate N — clean, plausible, and completely wrong.

Measured MK2 latencies, 5/5 constant trials each:

| probe shape | velocity word | pointer x |
|---|---|---|
| neutral walk (both ports) | 1 | 2 |
| attacker probe (walk from recovery) | 1 | 2 |
| **guarded defender (release Block + walk)** | **10** | **11** |

`input_latency_frames` is subtracted from every raw probe result and stored
alongside the data, per observable AND per probe shape. An uncalibrated run
is not a run.

**Calibrate at TWO probe points and require agreement.** A single point taken
too close to contact measures residual stun as latency: far HK's defender
calibrates to 6/7 at anchor+40 but 1/2 at +70 and +100. The near value would
have inflated `on_hit` by 5 frames, silently and plausibly.

## 4. The act-again probe — the measurement protocol

The key property: **no hit-state byte is required.** "Can this fighter act"
is observable as a behavioural divergence between two otherwise-identical
replays.

### 4.1 The anchor

`contact` is the frame the port's contact signal fires.

- **MK2 arcade**: the **fighter-struct health `block+0x0E`**. NOT the HUD
  pair `0xBCA0`/`0xBC88`, and NOT `hit_counter 0xD3FE`.
  - `0xD3FE` does not move for hits landed on P2 (P1-victim only, possibly
    1P-mode-only) and is not in the shipped profile.
  - The HUD pair is the **drawn bar**, which ANIMATES toward the true value
    at 1 unit/frame. One HP produced **11 consecutive changes** (161→150),
    so the "last contact before the quiet window" rule clustered them into
    9–11 "hits" and anchored the contact **10 frames late**. The first draft
    named it because it copied the profile's globals instead of reading
    mk2.md's own correction.
  - `block+0x0E` steps by the whole damage in ONE frame (161→150 on hit,
    161→158 on block) and yielded `contact=55, hits=1` on every trial.

  Blocked contact chips −3/−6, so the anchor fires for hits AND blocks; a
  NO-change trial is a WHIFF.

  **General rule this exposes: never anchor on a DRAWN value.** Anything the
  game animates toward a target reports a smear of edges where the event had
  one. Anchor on what the game computes, not on what it displays.
- **Genesis**: no contact signal exists (a confirmed negative, not an
  unexplored gap). Advantage is therefore **unmeasurable** on Genesis until
  one is found, and rows are NULL with that reason. Do not substitute a
  proxy.

**Multi-hit moves**: consecutive contacts inside the counter's ~20-frame
window do not reset it, so anchoring on the FIRST fire while the defender's
stun is set by the LAST makes advantage too negative by the inter-hit gap.
Anchor on the LAST contact before the quiet window, and store `hits`.

### 4.2 The observable — differential, never absolute

```
probe_run   : load state → step to anchor+N → HOLD walk → step W frames
control_run : load state → step to anchor+N → hold NOTHING → step W frames
actionable(N) := observable(probe_run) ≠ observable(control_run)
```

**W is per-observable and is not free.** W must be that observable's own
`input_latency_frames` plus a shared margin — NOT one shared W. With a single
W, two observables with different manifestation margins disagree by a
constant forever, which is exactly the §8.4 failure the cross-method check
exists to catch.

The differential form is mandatory, and it is what makes the whole protocol
sound. **State it as a law, because it has now bitten this project three
times in one wave:** on a game where things move on their own, no ABSOLUTE
observation of motion means anything. The three instances —

1. the first draft of this contract, where pushback moving `x` after contact
   would have reported the defender actionable during blockstun;
2. the arena liveness probe, whose absolute "did this port move while I held
   a direction" test reported the CPU-driven port of a 1P-vs-CPU rig as a
   live human port — the exact rig confusion that wasted two earlier
   sessions;
3. `action_counter` (`+0xC0`), accepted as a contact signal in a rig with no
   whiff control, then retracted.

Every one was fixed the same way: run the identical scenario with and
without the input, and believe only the difference. Any new probe in this
project starts from that shape. An absolute test ("did x change?") reports TRUE during pushback,
during hitstun animation, and during any scripted motion — none of which
mean the fighter has control. Differencing against an identical no-input
replay cancels pushback, hitstop, and animation churn in one stroke.

Observable choice is PER PORT and must be established by measurement. The
first draft's preference order was inverted on MK2 arcade; what follows is
measured.

1. **A narrow velocity/motion field**, where one exists. On MK2 arcade the
   walk-velocity word `block+0x0B..0x0D` (`00 00 00` standing, `00 fe ff`
   walking). Not yet in any profile; it should get a fighter-field slot.
2. **Pointer-resolved `x`** (`obj+0x12`) — sound, and valuable precisely
   because it lives in a DIFFERENT data structure from (1), which is what
   makes the §8.4 cross-method check meaningful rather than two views of the
   same bytes.
3. **Whole-fighter-struct divergence** — only where demonstrated. On MK2
   arcade it is CONTAMINATED by an input echo: probing a defender deep in
   blockstun (N = 3, 11, 19, 26, 40, where the answer must be FALSE)
   diverged from control within 1–2 frames at EVERY N, in `+0x1C`, `+0x6C`,
   `+0x70..0x72`, `+0xC0`, `+0xC4..0xC6` — bytes echoing the raw held
   direction while the fighter is stunned. The earlier "a guarding fighter's
   struct is entirely frozen, idle churn 0 bytes" result was an ABSOLUTE-test
   observation and does not transfer to a differential probe. That is the law
   above, applied to itself.
4. **`action_counter` (`+0xC0`)** — **fails outright** as an act-again
   observable on MK2: zero divergence for a held walk, both ports, both
   directions. It fires on entering an ATTACK (160→192), not on regaining
   control.
3. **Mapped `x`** — Genesis `+0xD8` (a stable struct field), and on MK2
   arcade the pointer-resolved `obj+0x12` (§5). The raw globals
   `p1_x`/`p2_x` remain **FORBIDDEN**: they read a frozen value through
   holds that visibly moved the fighter, because the pool slot they name is
   not stable. Resolve through `block-0xC` instead, and cross-check
   `obj+0x3E` against `block+0x0` — a mismatch means the pointer went stale
   and the row must be discarded, not recorded.

The probe is a WALK, never an attack: attacks buffer, get absorbed by held
Block, and are re-resolved by proximity into different moves.

**Blocked-direction hazard** (generalises the corner case): a fighter cannot
walk into a wall, and cannot walk into the OPPONENT'S BODY either — at
contact range the attacker's forward and the defender's forward are both
dead. Try both directions, and choose the direction **per observable**: a
noisy observable otherwise locks in a direction a clean one cannot use. If
neither diverges, record NULL rather than "never actionable".

### 4.3 The search

- **Linear sweep from N=0 is the DEFAULT method.** `MAX_SEARCH` is ~60
  frames; the cost is bounded and the result is unconditionally correct.
- **Binary search is opt-in**, permitted only where `actionable(N)` has been
  demonstrated monotone for that move class, and the `method` column records
  which was used. Monotonicity is not assumable: the first draft's
  N−1/N/N+1 confirmation does not detect a predicate of shape T…T F…F T…T,
  which is exactly what an absolute (non-differential) observable produces.

Advantage is then two probes from one anchor:

```
advantage = actionable(defender, contact) − actionable(attacker, contact)
```

**Two rules learned by publishing a number that was wrong by 9 frames:**

1. **State the convention.** This lab measures "the earliest frame this
   fighter can START A WALK". That is not "can attack" — measured on MK2,
   earliest-attack = walk-manifest − 2. Any published number must name its
   convention, because a reader comparing against a community table is
   comparing against a different one.
2. **Never subtract per-side calibration when the two sides have DIFFERENT
   probe shapes.** Latency cancels out of a difference only when the shapes
   match. On block they do not (attacker 1, guarded defender 10), and the
   defender's 10 is measured while FREE — during blockstun the block-stance
   drop runs concurrently, so subtracting it made every move look 9 frames
   more punishable. Difference the raw MANIFEST frames.

**Knockdown must gate the on-hit measurement.** The probe will happily
return an `on_hit` for a launcher, and it is meaningless (§1.1: a knockdown
has a wakeup clock, not an advantage). Detect it from the victim's own
resting `y` — §10 forbids a scalar GROUND_Y here — and record `on_hit` as
NULL with `knockdown` set.

**A move must be identified by its measured SIGNATURE, not by the buttons
pressed.** `down+button` on a single frame enters something that connects at
no gap; concluding "crouching normals never reach" from that is clean,
plausible and false. Validate that the intended move actually came out
(damage, contact frame, reach) before recording a row against its name.

`on_hit` and `on_block` are two runs of the identical protocol differing
only in whether the defender's guard is held. They are separate columns and
MUST NOT be derived from each other.

### 4.4 Where FAF comes from

FAF is NOT produced by the actionability probe. It is measured separately:
**the first input-relative frame at which the contact signal fires at the
MINIMUM REPRODUCIBLE GAP.** The first draft said "gap 0", which is not
reachable: MK2's anti-overlap collision resolution imposes a floor (measured
~62 px for a Reptile mirror — walking further closes nothing). The stored
row therefore records the gap it was measured at; "point-blank" is a
measured value per matchup, never an assumed zero. At larger gaps the
measurement is contaminated by travel and hurtbox extension, so FAF is
stored only for the minimum-gap row and is NULL elsewhere. Variant discrimination (§5) therefore
cannot key on FAF — it keys on measured differences in damage, advantage, or
connect behaviour at explicit gaps.

## 5. The spacing ladder

Proximity normals mean the same input is a DIFFERENT MOVE by distance, and
projectile advantage is a curve over gap rather than a scalar. So gap is
part of the key, and arenas are generated, not hand-saved.

**MK2 arcade CAN now measure gap in pixels** (this reverses the first
draft's limitation). The fighter struct carries a pointer to its object-pool
entry at `block - 0x0C`:

```
obj = (u32_le(block - 0x0C) - 0x01000000) >> 3   # TMS34010 bit-address
x   = u16_le(obj + 0x12)      # world X, 1 unit = 1 pixel
y   = s16_le(obj + 0x16)      # SIGNED; smaller = higher
cid = u8   (obj + 0x3E)       # MUST equal block+0x0 — staleness check
```

Verified across cold boots and a mid-session pool move (`0x69D2 → 0x6D2C`)
that the pointers followed while the fixed globals went to garbage; both
fields are write-authoritative. Record walk-frames as well as pixels: K
walk-frames from a fixed reset is reproducible without any position read and
remains the fallback if a pointer resolves invalid.

1. Reset to a known position.
2. Walk K frames; save `shadow/arenas/<family>/gap-K.state`. **Note the
   orientation: for a ladder that walks TOWARD the opponent, K=0 is the
   FARTHEST rung, not point-blank.**
3. Sidecar records: K, the achieved pixel gap if trustworthy, both char ids,
   facing, and `inputs_live` for BOTH ports.

Minimum ladder: point-blank (K=0 after a reset that leaves them touching),
the move's connect boundary minus a margin, and one intermediate. Variants
are stored as separate `variant` rows — never averaged into one.

Facing is recorded with every arena because a side swap flips the sign of
everything gap-keyed (MACRO_ACTIONS §10.2). **On a port with no verified
facing field, facing is DERIVED from relative position** (sign of
opp.x − me.x) and the sidecar must say it was derived, not read — MK2
arcade has no confirmed facing byte (`0xBE81` reads constant through a
crossover; `obj+0x18` does not flip). Where position itself is unmeasurable
the facing is NULL, never a guess.

**Sidecar filenames must not collide with the app's own.** Saving any state
under `shadow/arenas/` makes the app write its own `<name>.meta.json` in the
same call. A harness that also wants `<name>.meta.json` will be silently
clobbered by whichever write lands last; use a distinct suffix. This matters
because the app's auto-sidecar can be WRONG while a harness's is right — a
live arena was auto-stamped `inputs_live: {p0:false, p1:false}` because the
profile's `x` still pointed at the disproven globals.

**Measured K → gap for MK2 (Reptile mirror, P1 walking toward P2):** the
relationship is NOT linear — a ~1.6 px/frame startup ramp, a ~2.5 px/frame
cruise from K≈5 to K≈45, then a hard floor at 62 px. Do not fit a line
through it; record both units per arena and let the data be what it is.
Also: forward and backward walk speeds are ASYMMETRIC (+12 px over 6 frames
forward vs +5 px backward), so any liveness or calibration check shaped as
"hold a direction, undo it, expect to return to the start" produces false
negatives on this game.

## 6. Schema and storage

The frame table is a MEASUREMENTS STORE, not an address book. That is why it
is not a profile: profiles are a few dozen hand-verified constants reviewed
in a diff; this is thousands of mechanically-produced rows queried by cell
and re-measured whenever a core or ROM version changes.

- **Authoring store**: SQLite, owned by the Python harness
  (`shadow_train.framelab`), stdlib `sqlite3`, no new Rust dependency.
- **Consumption**: an exported `library/<family>/<port>.frames.json` that
  the app and Lua read. Rust never opens the database.

```
move_frames(
  family, port, char, move, variant, gap_walk_frames, gap_px,
  first_active_frame, active, recovery, total, hits,
  hitstop, on_hit, on_block, wakeup_window,
  knockdown, juggle, guard_height, connect_range,
  rig_guard_state, damage, observable, method,
  input_latency_frames, sample_n, confidence, measured_at,
  core_id, rom_id
)
```

`core_id`/`rom_id` are not bureaucracy: a frame number measured on a
different core build is a different number, and without them a stale row is
indistinguishable from a fresh one. `observable` and `method` are stored per
row because a row measured by struct-divergence and one measured by
`+0xC0` edge are different experiments.

## 7. Honesty requirements

Each of these is a mistake this project has already made once — the first
three within this document's own first draft.

- **Check the evidence doc before asserting a signal.** This contract shipped
  a draft naming a contact signal its own `library/<family>/<port>.md` had
  already retracted. A profile or evidence doc that contradicts a plan wins.
- **No silent caps.** If a run skips moves, gaps, or characters, the report
  names what was skipped and why.
- **A number that fails re-measurement is DELETED, not averaged.**
  Disagreement between runs means the protocol was wrong, not that the truth
  is in the middle.
- **Every row carries its method, observable, and calibration.** A row
  without provenance is the `action_counter` mistake in another costume.
- **External tables are cross-checks, never sources.** Record both and
  reconcile in writing; never quietly adjust ours to match. The leaked MK2
  source code circulating online is explicitly OUT OF SCOPE as a source —
  it is under an active DMCA takedown, and this lab measures.
- **Unmeasurable is a result.** "No contact signal on this port, so
  advantage is unmeasurable" is a legitimate table entry and must render as
  such — the Genesis contact-signal negative is the precedent.

## 8. Acceptance

Criteria 1–3 test PRECISION; criterion 4 is the only one that tests
ACCURACY, which is why it is mandatory rather than best-effort. A re-run of
the same protocol reproduces the same systematic error perfectly.

1. **Re-measurement is exact.** An independent re-run of a random sample of
   ≥5 rows reproduces them to the frame, from a cold start.
2. **Internal consistency holds** or is explained: `first_active_frame ≥ 1`,
   `total ≥ FAF + active`.
   **`on_hit ≥ on_block` is NOT one of these.** The first draft asserted it;
   MK2 arcade violates it legitimately and repeatedly. Blockstun there takes
   only two values across Reptile's whole kit (+19 close, +23 everything
   else) while hitstun varies per move, so far punches come out +4 on hit
   and +13 on block — confirmed independently by a punish rig (defender
   counters at contact+12 on hit, contact+21 on block). `on_hit ≥ on_block`
   encodes a modern-game convention, not a law. A checker may FLAG the
   inversion; it must not reject the row.
3. **A punish the table predicts actually lands.** Take the most unsafe move
   in the table, block it, punish it with the fastest normal the table says
   reaches: it must connect. Take one the table calls safe by ≥3 frames:
   the same punish must NOT connect.
4. **Cross-method agreement is REQUIRED.** At least one row per character
   must be measured a second time by a DIFFERENT observable (struct
   divergence vs `+0xC0` edge) and agree to the frame. Criterion 1 catches
   noise; only this catches a constant offset, and a 3-frame systematic
   error passes criteria 1–3 unnoticed.
5. **Sanity against an external table**, if one is obtained. Not required to
   match — required to be reconciled in writing.

## 9. Surfaces

- **Overlay (Lua)**: per-move connect-range markers, live gap readout, and
  the on-hit/on-block numbers for the last move thrown. These are MEASURED
  RANGE MARKERS, not hitboxes — we do not read the game's collision data,
  and the overlay must say so. Positional markers require a trusted position
  source, so on MK2 arcade the numeric readouts ship before the markers.
- **Panel**: the frame table for the loaded character, sparse cells marked
  the way the coverage grid marks them.
- **Live meter**: advantage computed at runtime from action-entry edges
  anchored to a contact event. Requires the defender to ACT for its edge to
  exist — i.e. the auto-reversal dummy is the measuring instrument, not a
  separate feature. A live reading lacking a defender edge renders "—",
  never 0.
- **MCP**: the harness drives `save_state`/`load_state`, `step`,
  `hold_buttons`/`release_buttons`, `read_memory`, and `run_lua`; no new
  tool is required to measure.

## 10. Stated limitations

- ~~MK2 arcade has no trusted position source.~~ **CLOSED** — the
  `block-0xC` object pointer (§5). Overlay markers and pixel gaps are
  unblocked.
- ~~MK2 arcade has no mapped `y`.~~ **CLOSED** — `obj+0x16`, signed, traced
  through a full jump arc. **But there is no scalar `GROUND_Y` for arcade**:
  resting y is character- AND stage-dependent (85 vs 83 on one stage, 89 vs
  87 on another), so "airborne" must be derived relative to that fighter's
  own resting y on that stage, never against a profile constant. Jump-in
  frame data is now possible on arcade, with that caveat.
- **Genesis has no contact signal**, so it cannot anchor. The two ports are
  therefore complementary and neither alone completes the table.
- **Charge and release moves** (Mileena's Sai) and **side-swapping moves**
  (her Teleport Kick) are not expressible in the macro DSL as it stands; see
  `MACRO_ACTIONS.md` §10.
- **Back-hold families are much harder.** MK2's block BUTTON keeps the
  defender static, which is what makes blockstun measurable at all.
  asurabld's back-hold guard walks the defender, which broke contact
  detection and spacing for the same root cause (MACRO_ACTIONS §9). Do not
  assume this protocol ports to asurabld unchanged.

## 11. Known gaps (open, dated 2026-08-30)

- **Arena sidecars cannot be re-probed without re-saving.** The gap ladder
  shipped with app-written `.meta.json` files asserting
  `inputs_live: false/false` — stale output of the pre-fix liveness probe —
  while the harness's own `.gap.json` correctly said `true/true`. The stale
  files were DELETED rather than shipped, because §3 makes the lab refuse an
  arena whose sidecar does not assert liveness, and a wrong sidecar is worse
  than an absent one. A `re-probe this arena` command would have fixed them
  in place; there isn't one.
- **The walk-velocity word `block+0x0B..0x0D` is not in any profile.** It is
  the cleanest act-again observable on MK2 arcade (§4.2) and currently lives
  only in the harness. It should get a fighter-field slot.
- **`first_active_frame` is NULL in every row measured so far.** §4.4 defines
  it, nothing has measured it yet.
- **Hitstop is unmeasured**, though §1.2 reserves a column for it.

## 12. Schema gaps found by the first kit run (open)

- **No arena / evidence column.** A row does not record which arena it was
  measured on, so a reader cannot reproduce it without reading the prose.
- **The export carries two rows per cell** (one per observable) with no
  guidance on which a consumer should pick. Either collapse agreeing
  observables into one row carrying both, or state the selection rule.
- **`hitstop`, `active`, `recovery`, `total`, `wakeup_window` and
  `guard_height` are NULL in every row measured so far.** The columns exist;
  nothing measures them yet.
