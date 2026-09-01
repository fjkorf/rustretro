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
             AND the defender's GUARD has not already returned
             AND the attacker is not holding a SPECIAL-MOVE input
```

The range clause is not a footnote. A move that is −8 but shoves the
defender past their own connect range is safe in practice.

**The FOURTH clause: MK2 arcade has PUNCH-LEAD special cancelling**,
measured 2026-08-31/09-01 on rev L3.1.

> **This clause was OVERSTATED when first written and is corrected here.**
> The run that discovered cancelling used only PUNCH leads and generalised
> to "specials come out at every frame from 2". A follow-up run added kick
> leads and the generalisation fails.

| lead | walk floor | special gate | margin | verdict |
|---|---|---|---|---|
| far HP (hitstop 0) | 20 | **2** | 18 | **CANCEL** |
| far HK (hitstop 12), whiff | 34 | 33 | 1 | **LINK** — inside §8.4 slop, refused |
| far HK, hit or block | 46 | 45 | 1 | **LINK** |

**Out of a kick there is no cancel at all, only a link.** A 1-frame margin
is inside cross-observable slop and the classifier's own `min_margin`
refuses it rather than rounding to an answer.

**Startup is UNCHANGED — the special starts later, never slower.** Measured
from the TRIGGER PRESS (not the macro start), Reptile's slide reaches
displacement in 3 frames and contact in 11 in every condition: no lead,
punch lead on hit/whiff/block, kick lead on hit/block/whiff, five gaps, and
again from a cold process. `force_ball` agrees at matched gaps. Cancelling
buys the lead's recovery, not a faster special.

**Hitstop is NOT bypassed, and this independently corroborates the hitstop
column.** Contact shifts the gate by exactly **+12** — the hitstop measured
for that move class by an entirely different method (whiff-differenced
attacker manifest). Two unrelated measurements agree to the frame: 33 → 45
on contact for hitstop-12 leads at two whiff gaps and two contact gaps, 0
shift for hitstop-0 leads. That is the independent-rig confirmation §8.4
cannot supply.

So the over-estimate is real but NARROWER than first stated: an
actionability number over-estimates commitment **for a fighter holding a
PUNCH-cancellable special's input**, not universally.

It also FALSIFIES the only published description (ded_'s GameFAQs combo
guide, disclaimed for our exact revision), which calls it a "Just Frame"
cancellable "on the frame of contact": the cancel needs no contact at all —
at 180 px where the lead normal WHIFFS entirely the slide's onset is
identical — and the window is open-ended, not one frame.

Confidence medium-high; §8 acceptance is NOT met — one character, one gap,
`sample_n = 1`, no cold re-run, and **nothing measured on block**, which is
the gap to close first. No `cancel` COLUMN is needed: this is a global
button-class rule, not a per-move property.

**The third clause was missing from the first draft and it changes verdicts.**
This lab measures "the earliest frame the fighter can start a WALK", but
GUARD comes back before the walk does — measured ≥7 frames earlier for
Mileena after cHK. So a negative advantage number is an UPPER BOUND on
punishability, not a verdict. Worked example: Mileena's cHK is −20 on block
and is **not punishable at the floor** — it shoves the blocker to 93 px,
outside punch range, and her guard is back before the kicks arrive. A table
that prints −20 and stops has told the reader something true and misleading
at once.

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
   capture — **and the probe's walk window must be long enough for the
   CHARACTER.** Baraka's walk cannot be aborted for 13 frames: released at
   hold ≤10 he travels a further +39 px over 12 frames with nothing held,
   where Mileena and Reptile stop within +3 px and 0 frames. The shipped
   6-frame probe therefore reported him NOT LIVE on a provably live port
   (FALSE at 6 and 10, TRUE at 14/16/20) — and §3 makes the lab REFUSE such
   an arena, so the default would have silently blocked his entire ladder.
   A liveness default tuned on two characters is a default tuned on two
   characters. The sidecar's `inputs_live` is a save-time assertion; the same
   object-pool instability that breaks `x` can invalidate it later.
5. **`step` is SYNCHRONOUS** — it returns only once the emulated frame is
   fully complete, and reports `landed`. No polling, no sleeping.
   (Historical: it used to be fire-and-forget, and 30 rapid calls landed
   **1** frame. That false negative briefly convinced an agent that held
   input cannot reach the core during stepping — it can. Measured after the
   fix: 0.91 ms/step, 200/200 landing, against a 41.1 ms baseline.)
   Use `run_frames(count)` to advance many frames in one call (~0.72
   ms/frame).
6. **Confirm the input FOLD, not just the write** — and, where a frame has
   already run, confirm what it EXECUTED. `get_input`'s `folded_*` is
   re-folded every host tick whether or not a frame ran, so a post-step read
   reports the current held set, not what the frame saw; `executed_*` is
   sticky and answers that. The fold oracle still waits on `folded_*`
   because it runs while PAUSED, before any frame exists — `executed_*`
   cannot report a frame that has not happened.

   Confirming what a frame executed is what separates a DIVERGED replay
   (§4.5) from a WHIFF. Without it, a replay that stopped feeding input
   looks exactly like a move that does not reach, and gets stored as one.

6b. **Confirm the input FOLD, not just the write.** Asserting a hold does
   not mean the core has seen it: the host loop folds input and checks the
   frame gate in separate lock acquisitions, so a batch armed in one
   acquisition can run its first frame on the PREVIOUS input. Poll
   `get_input` until `folded` equals what was asserted. This is an ORACLE,
   not a wait.

   **The 8 ms "settle" that used to live here is RETIRED, and it was never
   the mechanism.** Measured head-to-head on the same rig it was slightly
   WORSE than nothing (16/100 spurious with it, 7/100 without); the original
   A/B that justified it used 14 pairs per arm, far too small to see a ~7%
   effect. The real failure mode is subtle enough to fool the rest of this
   contract: the ATTACKER's move came out one frame late, contact moved
   f11→f12, and the defender was genuinely free on the probe frame — so both
   observables agreed with each other on the wrong answer and §8.4's
   cross-method check could not see it.

   | how the hold was asserted | spurious TRUE |
   |---|---|
   | batch per-port masks | 13 / 200 |
   | `hold_buttons` then batch | 0 / 200 |
   | `hold_buttons` + fold confirmation | 0 / 400 |

   After the fold oracle: 52 exhaustive sweeps, 2,626 repeat-checked
   evaluations, no settle — **0 repeat-check failures, 0 non-monotone
   refusals**.
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

  Blocked contact chips −3/−6, so the anchor fires for hits AND blocks. A
  NO-change trial is a WHIFF **only if the move actually came out** — with a
  replay source it may instead be NO-EXECUTE (§4.5), and the two must not be
  conflated: one says the move does not reach, the other says nothing was
  thrown.

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
advantage = manifest(defender, contact) − manifest(attacker, contact)
```

where `manifest` is the RAW frame at which that side's probe first diverges
from its control — **calibration is NOT subtracted here.** An earlier draft
of this formula used the calibrated `actionable` on both sides, which is the
9-frame error described immediately below; the two halves of this section
contradicted each other until now.

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

## 4.5 Replay-sourced measurement

A move may come from a synthesized script OR from a recorded input slot
(`record_inputs`/`play_inputs`, `shadow/inputs/<family>/`). A replay is a
real execution and connects the lab to actual play — but **a slot is valid
only against the state it was recorded from**, so its timing is a hypothesis,
never an anchor.

**The rule: a row anchors on OBSERVED contact, never on the replay's
expected contact.** Classify, and refuse what cannot be vouched for:

| classification | meaning | row? |
|---|---|---|
| `ON-TIME` | contact at the expected frame | yes |
| `RETIMED` | contact at a DIFFERENT frame | **yes** — re-anchored, delta recorded |
| `WHIFF` | move came out, no contact | no (a valid result) |
| `NO-EXECUTE` | no attack signal at all | no — refusal, counted |
| `DIVERGED` | executed input stream ≠ the slot's | no — refusal, counted |

RETIMED is not an error. Measured live: a Reptile HP slot recorded at 72 px
is `ON-TIME` there and `RETIMED` by **−3 frames** at the 62 px floor, because
at that distance the same transcript is the CLOSE HP — a different move by
proximity. Anchoring on the recording's frame would have put the whole sweep
3 frames off with nothing in the row to say so.

**Divergence is an INPUT test, not a state test.** A different arena is
supposed to produce a different state trace; what must match is the executed
input stream against the slot's own masks. The state-trace comparison belongs
only in the determinism check below.

### 4.6 Determinism is a session alarm, not a failed cell

Replaying one slot twice from one state must produce identical traces. A
failure means every measurement on that session is suspect — it is a SYSTEM
ALARM, reported separately from any cell's result, and its scope must be
recorded (a narrow all-clear must never overwrite a wide alarm).

Measured, and it found a real defect — now FIXED. `resume → load → poll →
pause` let an uncapped core run a VARIABLE number of free frames inside the
window (10–15 over 16 loads), so every "identical" replay started from the
saved state plus a variable-length prefix. Any frame-counting field recorded
it (`block1+0x1C == free_frames + 1`, no exceptions). Whole-struct scope
alarmed 12/16 in one measurement and 16/16 in another; the profile's active
observables 0/16 — which is why no shipped number was corrupted, and that was
luck rather than design.

**The fix is `load_state(pause_after: true)`**: the load and the pause happen
in the same lock scope on the emulation thread, so the caller gets back an
emulator paused at exactly the loaded state. Measured after: free frames
`[0]×16`, whole-struct determinism 0/16 alarms. **The lab must never bracket
a load with `resume`/`pause` again** — plain `pause` remains fire-and-forget
(it sets a flag without confirming the in-flight frame finished), and a
`pause_after` load following a plain `pause()` was observed picking up a
stray free frame. Never a sleep.

## 5. The spacing ladder

Proximity normals mean the same input is a DIFFERENT MOVE by distance, and
projectile advantage is a curve over gap rather than a scalar. So gap is
part of the key, and arenas are generated, not hand-saved.

**The collision floor is PER-MATCHUP, not per-port.** Reptile's mirror
floors at 62 px, Mileena vs Reptile at 61 px, and she walks ~25 % faster
with no startup ramp — so one character's K→px curve misses every target gap
for another. Generate a ladder per matchup; never inherit one.

**Two hazards in the ladder recipe itself**, both found only after the first
ladder shipped:
- **The liveness probe WALKS both fighters.** The shipped Reptile "K=0" rung
  reads 180 px from a 192 px base — that 12 px is the probe, not the walk.
  Probe liveness in a separate pass, or re-read the gap afterwards.
- **The gap OSCILLATES inside the floor** while a direction is held (60–66,
  settling at 63), and walking past the floor *opens* it again. A rung saved
  mid-walk samples the oscillation; settle before saving.

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

**Momentum makes the settled K→gap curve discontinuous.** Baraka's is
non-monotone across his abort boundary — K=10 settles at 126 px, K=11 at
159 px (`156−3K` below the break, `192−3K` above) — so a generator assuming
monotonicity picks the wrong rung. `settle_frames=8` is not enough for a
character with momentum; his run used 20. Third matchup, third collision
floor: 62 / 61 / 63 px.

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
  names what was skipped and why. **This rule was violated by a DEFAULT, not
  by a decision**: `DEFAULT_ANCHOR_FRAMES = 48` minus a 20-frame quiet window
  leaves a 28-frame contact horizon, and Baraka's close LP is a THROW
  contacting at frame 40 — 34 damage, unblockable, knocks down. The connect
  map printed `—`, **the same glyph a genuine whiff gets**, so an entire move
  read as "does not reach". A 90-frame re-scan found it.
  **Retroactive: Reptile's throw contacts at f48 and is also outside the
  default horizon**, so his and Mileena's connect maps must be re-scanned at
  90 frames before their tables are called complete. A cap that renders
  identically to a measurement is the most dangerous kind.
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
3. **A punish the table predicts actually lands.**
   **Protocol correction — the first draft's version cannot be run.**
   Dropping Block and pressing the counter on the SAME frame produces no
   attack at all (all four buttons, every N from +8 to +30, `action_counter`
   never moves), so a naive punish rig reports EVERYTHING as safe. Release
   the guard at least one frame before the counter. This is the same
   same-frame-chord rule that governs move inputs (MACRO_ACTIONS §11) —
   guard release obeys it too. Take the most unsafe move
   in the table, block it, punish it with the fastest normal the table says
   reaches: it must connect. Take one the table calls safe by ≥3 frames:
   the same punish must NOT connect.
4. **Cross-method agreement is REQUIRED — but "agree" means something
   different depending on what kind of quantity the row is.** At least one
   row per character must be measured a second time by a DIFFERENT
   observable (struct divergence vs `+0xC0` edge, or `struct_velocity` vs
   `pointer_x`), and:

   The class is **not a property of the field's name** — it is a property of
   whether the value carries a probe manifest's manifestation margin, which
   depends on how the row was actually measured. Three shapes, not two:

   - **DIFFERENCE quantities agree to the frame, exactly.** `advantage`
     (`on_hit`/`on_block`) is a difference of two manifest frames measured
     with the SAME observable (§4.3's `manifest(defender) − manifest(attacker)`),
     so that observable's own manifestation margin appears in both terms and
     cancels out of the subtraction. **`hitstop` is also a difference
     quantity** — connecting manifest minus whiffing manifest — even though
     it is a duration: the margin cancels the same way advantage's does, so
     it is held to the same exact-frame rule and must NOT be given the
     one-sided leniency below. It has agreed exactly on every cell measured
     so far, across three characters.
   - **ANCHOR-BASED quantities are bracketed by two reads of the SAME anchor
     signal (§4.1), never a behavioural probe**, so neither endpoint carries
     a manifestation margin and they too are held to exact agreement —
     for a different reason than a difference quantity (no margin ever
     entered the number, rather than two margins entering and cancelling).
     `first_active_frame` (the contact signal relative to a fixed,
     software-controlled input frame) and `active` (the first and last
     contact-signal reads across a gap sweep) are both this shape.
   - **ONE-SIDED quantities carry the observable's margin directly, and
     legitimately differ by it.** `wakeup_window` (and any future raw
     single-sided manifest, e.g. an `actionable_after_contact` column) is
     `value = A_rel + m`, where `m` is that observable's own manifestation
     margin (§4.2's probe algebra) — the SAME `m` that is baked into that
     observable's own `input_latency_frames = l + m` at calibration (§3.1).
     Two observables measuring the same truth therefore differ, in general,
     by exactly the DIFFERENCE in their `input_latency_frames` — treating
     that as a disagreement is a units error in the check, not a finding.
     A one-sided row's two raw values must satisfy
     `value₁ − latency₁ == value₂ − latency₂`; anything else is still a real
     disagreement and must still be flagged.
     **`total` and `recovery` are this shape too, corrected 2026-09-01.**
     They were originally grouped with the anchor-based fields on the
     reasoning that they are "measured from an anchor rather than a probe
     manifest" — that reasoning holds for `first_active_frame` and `active`,
     which bracket against the contact signal itself, but not for
     `total`/`recovery`: under the only measurement protocol this project has
     (§4), "recovered" has no anchor signal of its own — it is read from a
     fixed anchor to the SAME act-again probe manifest a knockdown's
     `wakeup_window` uses, so it carries exactly one margin, identically.
     This is not a hypothetical: a WHIFF-anchored duration has no contact to
     bracket against at all, so its `total` can *only* come from the
     act-again probe. Reptile's invisibility does no damage and reads
     `total` 40 (`struct_velocity`, latency 1) / 41 (`pointer_x`, latency 2)
     — agreement under this rule (`40 − 1 == 41 − 2`), disagreement under the
     rule this document previously specified. See §13 item 1 (closed) for
     the blocker this caused.

   **Worked example — Mileena's roll.** `wakeup_window` reads 77 from
   `struct_velocity` (`input_latency_frames = 1`) and 78 from `pointer_x`
   (`input_latency_frames = 2`). `77 − 1 == 78 − 2 == 76`: this is exact
   agreement under the one-sided rule, not the 1-frame disagreement the
   difference-quantity rule would have reported. The collapsed value (77) is
   stored in the frame of reference of whichever observable has the SMALLEST
   `input_latency_frames` in the cell — by the probe's own construction that
   observable's margin `m` is zero, so its raw reading already equals the
   margin-free truth with nothing to correct. A reader MUST be able to tell
   which observable's frame a stored one-sided number is in; a bare "77"
   that means different things depending on which row it came from is
   exactly the ambiguity §7 exists to prevent.

   **This is a fact about the current protocol, not a permanent fact about
   these field names.** If a future observable can read "recovery ended"
   directly off an anchor signal (no probe involved) for a CONTACT-anchored
   move, that row would be anchor-based, not one-sided — and today's schema
   has no column to say which shape a given row is; the loader infers it
   from the field name because every row this project has produced so far
   follows the current protocol. The honest fix is a per-row column (e.g.
   `anchor_kind: "dual_anchor" | "anchor_to_probe"`) so collapsing reads the
   shape off the row instead of assuming it from the field's name — proposed
   in §12, not yet implemented: nothing populates it (that's the Python
   harness's job, out of scope for a Rust-only change), and a column nothing
   ever writes is not a real fix.
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
- **MCP**: the harness drives `save_state`/`load_state`, `step` (now
  synchronous), `run_frames`, `hold_buttons`/`release_buttons`,
  `get_input` (the fold oracle of §3.6), `read_memory`, and `run_lua`.
  An earlier draft claimed no new tool was required to measure; `run_frames`
  and `get_input` are both required now.

## 10. Stated limitations

- ~~MK2 arcade has no trusted position source.~~ **CLOSED** — the
  `block-0xC` object pointer (§5). Overlay markers and pixel gaps are
  unblocked.
- ~~Jumping normals are unmeasurable — the probe is a walk and an airborne
  fighter cannot walk.~~ **REFUTED 2026-08-31.** The premise is true and the
  conclusion false: the probe is not blind mid-flight, it is DEFERRED to
  after landing. No new observable was needed — both existing ones worked
  unchanged and agreed on all 70 sweeps. Three gates were: an AIR-CONTROL
  SCAN (hold every direction at every airborne frame, differenced against a
  no-input control, window capped at landing — **0 divergences in 152
  evaluations per arena**), a calibration point derived from each run's own
  landing rather than a constant, and a refusal for any boundary landing
  before the fighter does.
  Note why that scan had to be run explicitly: **neither of this contract's
  safeguards can see air control.** Differencing does not protect you (the
  control is not drifting) and §8.4 cannot either (both observables would
  move together). It took a dedicated experiment.
  Measured: Reptile's neutral jump HP swings **−2 → +3 on block across one
  arc** — 4 frames, same move, by contact height alone — decomposing to
  `landing + 7` on every row, the same `landing + 7` Mileena's airborne
  teleport gave. Gap-independent.
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
- ~~The walk-velocity word `block+0x0B..0x0D` is not in any profile.~~
  **CLOSED** — it, the anchor, the observable preference order, the probe-shape
  calibrations and the collision floor now live in the port profile's
  `framelab` block (`docs/game-profiles.md`). An uncalibrated port DECLINES
  with a named reason instead of inheriting MK2's numbers.
- ~~`first_active_frame` is NULL in every row.~~ **CLOSED** — the shipped
  export carries 8 / 11 / 8 for the 62 px rows, and 12 for cLK.
- **Hitstop is unmeasured**, though §1.2 reserves a column for it.

## 12. Schema gaps found by the first kit run (open)

- **No arena / evidence column.** A row does not record which arena it was
  measured on, so a reader cannot reproduce it without reading the prose.
- **The export carries two rows per cell** (one per observable) with no
  guidance on which a consumer should pick. Evidence now strongly favours
  collapsing: **zero disagreements across three full runs and two
  characters** (52 sweeps in the second, 94 in the third). The Rust loader
  already collapses field-by-field and names any disagreement. Collapse into one row carrying both,
  and treat a DISAGREEMENT as the exceptional case it has never yet been.
  ~~**RESOLVED — the first real cross-observable difference arrived, and it
  was not a measurement error.**~~ Mileena's roll produced `wakeup_window`
  77 (`struct_velocity`) vs 78 (`pointer_x`), and the loader flagged it as
  the first-ever disagreement. It was not one: the two observables have
  different calibrated `input_latency_frames` (1 and 2 — §3.1), and
  `wakeup_window` is a ONE-SIDED quantity that carries that margin directly
  (§8.4, corrected). The bug was a **units error in the check itself** —
  applying the exact-frame rule (correct for `on_hit`/`on_block`, which are
  DIFFERENCES and cancel the margin) to a quantity that doesn't cancel it.
  Fixed in `src/profile.rs`'s `collapse_measurements`: difference/anchor
  fields still require exact agreement; one-sided fields require agreement
  after subtracting each observation's own latency, and record which
  observable's frame of reference the collapsed value is in
  (`FrameCell::one_sided_reference`). Zero disagreements now holds again
  across all 41 shipped cells.
- **`hitstop`, `active`, `recovery`, `total`, `wakeup_window` and
  `guard_height` are NULL in every row measured so far.** The columns exist;
  nothing measures them yet.
- **Proposed: a per-row `anchor_kind` column** (§8.4, from closing §13 item
  1). The collapse rule currently infers whether a field carries a probe
  manifest's margin from the FIELD'S NAME — correct today only because every
  row this project has produced follows the one measurement protocol this
  document describes (§4). `anchor_kind: "dual_anchor" | "anchor_to_probe"`
  (naming the two shapes in §8.4) would let a future row declare which shape
  it is, so a duration measured a genuinely different way — e.g. a future
  observable reading "recovery ended" directly off an anchor signal for a
  contact-anchored move, with no probe involved — collapses correctly
  without a code change. Not implemented: it must be populated by
  `shadow_train.framelab` at export time, which is out of scope for the
  Rust-only fix that closed §13 item 1, and an unpopulated column is not a
  real fix — `src/profile.rs`'s `collapse_measurements` still infers the
  shape from the field name (`total`/`recovery` now join `wakeup_window`;
  `first_active_frame`/`active` stay with `on_hit`/`on_block`/`hitstop`).

## 13. Open contract gaps (2026-09-01, from the specials/airborne/cancel runs)

1. ~~**§8.4 misclassifies a whiff-anchored `total`/`recovery`.**~~ **CLOSED
   2026-09-01.** `total`/`recovery` are read from a fixed anchor to the same
   act-again probe manifest `wakeup_window` uses, under the only measurement
   protocol this project has (§4) — that holds regardless of whether the
   anchor end was a contact or a whiff, so both fields are ONE-SIDED, not
   anchor-based, unconditionally. Reptile's invisibility reads 40
   (`struct_velocity`)/41 (`pointer_x`) — agreement under the one-sided rule
   (`40−1 == 41−2`), the disagreement the old rule reported. Fixed in
   `src/profile.rs`'s `collapse_measurements` (`total`/`recovery` moved from
   `exact_field!` to `one_sided_field!`); `hitstop` was checked separately
   and correctly stays exact — it is a DIFFERENCE of two manifests
   (connecting − whiffing), not a probe manifest itself, so its margin
   cancels the way advantage's does. The residual gap — that this
   classification is inferred from the field name rather than declared per
   row — is tracked as the proposed `anchor_kind` column in §12, since
   nothing populates it yet.
2. **No way to record "measured, nothing to report".** Invisibility is fully
   measured — 0 damage, 0 contacts in 200 frames, 3/3 — and produces no
   storable row, so a reader cannot distinguish it from never-measured. The
   overloaded-absent problem (§7) at cell granularity rather than field.
3. **The cap rule (§4.3/§7) is stated only for the UPPER edge of a search.**
   `force_ball` recovers BEFORE its own projectile lands, so a
   contact-anchored sweep returns `first_true = 0` instead of a negative —
   plausible, and silently 5 frames late.
4. **§4.2's blocked-direction hazard has a DYNAMIC form.** A launched victim
   landing on the attacker is pushed apart by anti-overlap at exactly walking
   speed, so probe and control produce identical `x` for 5 frames — a
   *differential* collision, where the game itself moves the fighter at the
   probe's own speed. And a fighter being separated from an overlap is
   genuinely not actionable, so §4.3's "non-monotone means a one-frame-early
   hold or an unsound observable" is not exhaustive.
5. **A move's signature is NOT gap-invariant**, though §4.3 implies it is: the
   slide travels 112 px from 182 px away and 62 px from 107 px, because it
   stops on contact. A threshold tuned at one rung refuses a correct move at
   another.
6. **§5's settle advice understates its own consequence.** A mid-walk rung
   does not merely shift pixels — it leaves a direction LATCHED, so a motion
   special's first tap is not a fresh onset and the special does not come
   out. The lab then measures the NORMAL that does, under the special's name.
   This is the THIRD distinct cause of the `acid_spit` failure mode. No
   sidecar field records whether the fighter was at rest.
7. **Travel is a STEP function of gap, not a ramp** — interpolating a
   projectile's advantage between measured rungs invents numbers.
