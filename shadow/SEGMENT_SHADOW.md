# Segment Shadow — a KI-style replay-segment opponent, and the organic arcade flow

**Status:** DESIGN (rev. draft-1, 2026-09-01). Not yet implemented. This is a
contract to be reviewed the way `docs/frames.md` and `shadow/SPEC.md` were,
before any code. Sections marked **[PENDING PROBE]** depend on the live
challenger-interrupt probe (`challenger-probe-evidence.md`) and must not be
frozen until it lands.

Companion docs: `shadow/SPEC.md` (the existing decision-level behavioral-
cloning shadow — this one runs ALONGSIDE it, not instead), `RECORDER_V3.md`
(the on-disk record format both consume), `MACRO_ACTIONS.md` (the DSL the
decision shadow needs and this one does NOT), `docs/game-profiles.md` (where
per-game data lives).

---

## 0. Why this exists, and why it is a SECOND engine not a replacement

The shipped shadow (`SPEC.md`) is **decision-level behavioral cloning**: it
cuts play into 8 Hz decisions, learns `state → (move, attack)` as a kNN case
store (with soft retrieval + a neutral cap), and re-synthesizes intents at
deploy. It generalizes to states the user never visited — its strength — and
it needs an OFFLINE FIT (`python -m shadow_train fit`) before it can fight,
and a macro DSL to reproduce specials.

Killer Instinct's Shadow (2013; Iron Galaxy, GDC 2016 talk by Neal & Hayles —
research on file) took the other road: **case-based reasoning over raw replay
segments**. No trained model. The shadow re-plays frame-exact CHUNKS of the
player's own recorded matches, chosen in context by weighted nearest-neighbor,
with a stickiness bias that keeps a segment playing until the world diverges.
Its properties, all sourced from the creators:

- **Functional in minutes** — 3 dojo matches for a baseline; no fit step,
  because the "learning" is just retention of annotated segments.
- **Structurally cannot do what the user didn't do** — specials, execution
  quality, dropped inputs, reaction-time limits all come for free from
  replaying real input streams. "The most human result is by definition
  whatever a human did in this circumstance."
- **Fails off-distribution** — a never-seen situation produces incoherent
  behavior until examples are recorded (their on-stage demo showed exactly
  this).

The two engines are complementary and share the SAME recordings and the SAME
feature layer. This engine is what makes the **organic arcade flow** (§7)
possible at all: a challenger that interrupts a live arcade run needs a shadow
that learned DURING that run, which a no-fit segment engine provides and an
offline-fit kNN engine does not.

**Non-goals** (inherited from SPEC §6, restated): optimal/superhuman play; a
reward signal; self-play. The product is "that's me", not "that's hard".

---

## 1. What a SEGMENT is — and the capture we already have

A segment is a reference into an existing recording, never a new capture
format:

```
Segment {
  file: recording path (jsonl-v3),
  start: frame index,      # inclusive
  end:   frame index,      # exclusive
  side:  which block was the demonstrator (P1 anchor, SPEC §5),
  start_features: the §3 similarity vector sampled at `start`,
  tags: {boundary_kind, char_id, opp_char_id, matchup_key, recency_rank},
}
```

The recording already stores, per 60 Hz frame, both fighters' raw struct
fields AND both raw 12-bit input masks (`RECORDER_V3.md`). That is precisely
what segment playback needs: the input masks to re-inject, the state rows to
compute divergence against. **No recorder change is required** for a first
version — segmentation is an offline/online pass over frames we already write.
**But only a side with a live HUMAN mask yields a segment:** in human-vs-CPU
recordings `p2_input` is all zeros (SPEC §5) — the CPU drives P2 internally,
not through the pad — and zero input mass is the project's own DEFINITION of a
non-demonstration (the demo filter). So arcade-run recordings yield P1-side
segments only; either-side segments require a human-vs-human corpus. A
divergence computation over MK2 rows must also honor ABSENT `via` fields
(x/y omitted, never zero-filled, on stale-pointer frames) — absent is not
zero — and side attribution rests on the block1=P1 fixed-slot anchor.
(A later refinement may add a segment-boundary annotation column, the way
`MACRO_ACTIONS.md §3` added `p1_special`; not needed to start.)

KI recorded EVERYTHING including locomotion and explicitly refused to
compress it (compression requires modeling intent — footsies vs approach —
and breaks frame-exact combo timing). We inherit that rule: **segments tile
the whole controllable stream with no gaps and no dedup.**

---

## 2. Segmentation — where one segment ends and the next begins

Boundaries are placed at events keyed on ADDRESSES we already have (no new
RE) — but "no new RE" is not "already implemented": segmentation is a NEW
offline/online pass, and the review flagged that three of these detectors are
specified or live-rig-only, not present in any recording pass. Honest status
per boundary:

- **Contact** — the shipped `contact_signal` (struct-health decrease, MK2).
  The recorder already reads it (`record.rs`); `dataset.py` does not (fine —
  segmentation is a new pass). READY as a signal.
- **Round edge** — the `gate` false→true / true→false. Implemented
  (`gate.rs`), and gate rev-3 fixed the 2-human leak. READY.
- **Knockdown / wakeup** — SPECIFIED (frames.md §4.3, victim resting-y
  transition) but implemented ONLY as a framelab LIVE probe
  (`framelab/airborne.py`), with a per-character/per-stage resting-y that has
  no scalar GROUND_Y. NEW CODE: an offline per-round resting-y estimator that
  tolerates ABSENT y (MK2's `via` field). Not "already detectable".
- **Neutral re-entry** — a `dist_x` hysteresis over recorded x. NEW CODE —
  `_bucket()` computes air/corner/hitstun only, NO gap dimension; do not cite
  it. On MK2 the estimator must handle ABSENT x.
- **A hard cap** — no segment longer than `SEG_MAX` frames (so a passive
  stretch still yields retrievable units; mirrors SPEC's SEGMENT_DECISIONS).

Every boundary kind is a profile/calibration constant, not a hardcode. A
segment carries its boundary_kind so retrieval can prefer like-for-like (a
wakeup situation retrieves wakeup segments).

**Overlap rule:** segments may overlap (a sliding window), OR tile disjointly
— decide during implementation against the replay-classifier validation (§8);
KI's "jump around in the replay" implies overlap is fine. State the choice in
the sidecar.

---

## 3. Similarity — the feature vector and the weighted metric

Reuse SPEC §1a's side-agnostic vector as the BASE (it is already normalized,
already forward-relative, already computed by `dataset.py`). KI used "40+
metrics"; ours is ~21 scalars + categoricals — the gap is mostly KI's
per-character state (knife positions etc.) and the trend features below.

**Three deliberate departures from SPEC's kNN, all sourced from KI:**

1. **Weighted sum (L1-style), not uniform L2.** KI combined per-feature
   similarities with DESIGNER weights, and confirmed categorical states carry
   very high weight ("is he standing or knocked down — the weight is very
   high"). SPEC's kNN uses standardized Euclidean where a binary `me_hitstun`
   is one of 84 stacked dims and gets swamped. Here, weights are DATA (a
   profile/model `similarity_weights` block), with knockdown / airborne /
   hitstun / corner weighted far above raw position. This directly attacks
   the genre's universal complaint (SPEC §7.3: every clone under-blocks
   because defense is decision-sparse and drowned in the metric).

2. **Trend features — the piece SPEC lacks entirely — but they are
   PER-GAME-AVAILABLE, not universal.** KI's state vector included RUNNING
   match statistics: opponent's high/low block ratio, reversal-on-wakeup
   frequency, its own fireball count. The Q&A is explicit that these are the
   shadow's only memory across segment switches, and what gives mid-match
   adaptation.
   **Correction from design review — the naive version is the classic
   cross-game error.** OPPONENT trend features (block-height ratio, reversal
   frequency) are NOT computable at RECORD time on MK2: every organic-flow
   recording is human-vs-CPU, and the CPU's input mask is permanently 0
   (SPEC §5), MK2 has no block-state flag and no facing field (mk2.md), so a
   CPU opponent's "block height" and "reversal frequency" are structurally
   zero — and worse, they would differ at DEPLOY, where the live human
   opponent's mask IS visible (a train/deploy mismatch). SELF trend features
   (my recent forward-hold fraction, my recent special/throw rate) ARE
   computable on MK2 from my own recorded mask. So: trend features are a
   profile-declared, per-game availability set (mirroring §4.2's feature-drop
   table) — SELF-trends everywhere; OPPONENT-trends only where the opponent's
   block/facing state is actually observed (asurabld's hold accumulators; any
   human-vs-human corpus), dropped-and-named on MK2 until new RE or an h-v-h
   corpus supplies them. The self-trend subset still benefits the existing
   kNN shadow (subject to §10(d)'s compat gate).

3. **Selection noise as KI framed it.** Not (only) temperature: KI found the
   best score then picked RANDOMLY among everything within ~10% of it. This
   is an A/B candidate against SPEC's temperature sampling (§8) — cheaper to
   reason about, and it is the shipped-in-KI unpredictability knob.

Categorical states (action index, char id) stay as SPEC §1a handles them.

---

## 4. Retrieval and the stickiness bias

Per decision tick (the ~8 Hz cadence, SPEC §4 — NOT per frame):

1. If a segment is currently playing AND not exhausted, compute the weighted
   deviation between the live world and the segment's recorded world at the
   corresponding offset. Add a **stickiness bias** to the score of "continue
   the current segment." Continue playing until deviation EXCEEDS the bias —
   then the segment "naturally breaks" (KI's exact mechanism) and we re-select.
2. On re-selection: weighted-NN over segment `start_features`, pick randomly
   within `SELECT_BAND` (~10%) of the best score, biased by `recency_rank`
   (recent data preferred — KI's answer to stale/patched data).
3. **No-match honesty:** nearest-NN always returns something, but if the best
   score is beyond `MATCH_FLOOR`, the shadow is off-distribution. Do NOT
   fabricate — surface it (a phase string / a live-meter "improvising" flag),
   the way SPEC's live meter renders "—" rather than 0. This is KI's
   documented incoherent-on-unseen behavior, made visible instead of hidden.

`stickiness_bias`, `SELECT_BAND`, `MATCH_FLOOR`, recency half-life: all
calibration DATA, tuned against §8, never hardcoded.

---

## 5. Playback — literal re-injection, and the one novel mechanism

A selected segment plays its recorded input masks, one per frame, re-injected
on the shadow's port (the existing per-port injection path, 2-frame holds).
Frame-exact — that is what preserves combos and the user's execution quality,
including drops (KI: "the exact frame timing of things are so important").

**The novel mechanism this project needs that KI did not: FACING-MIRRORING.**
KI's shadow replayed the player's own side. Ours replays a human-demonstrated
side (P1-side from human-vs-CPU; either human side from human-vs-human — a
CPU side has no input stream and yields no segment, §1) through whichever side
the shadow is on. So:

- **Facing is a PER-GAME source, and on MK2 it is DERIVED and can be ABSENT.**
  asurabld has a facing byte (SPEC §2, +0x61); MK2 has NONE (`p1_facing`
  0xBE81 was disproven and removed), so facing is `sign(opp.x − me.x)` through
  the object-pointer x — which is ABSENT (never 0) on a stale-pointer frame,
  and whose sign is undefined/oscillating exactly at a cross-up (opp.x − me.x
  near zero). The mirror must define recorded-facing and live-facing per game
  and state an absence/tie policy (hold last known facing; never synthesize 0).
- **Pin at segment start — do NOT resolve per frame (corrected from
  review).** `MACRO_ACTIONS.md §10.2` is explicit that per-frame facing
  resolution is "correct frame by frame and therefore WRONG across a swap":
  the fix is to LATCH facing at the segment's start frame and apply that one
  sign across the whole segment, re-latching only at a debounced, CONFIRMED
  side swap — not a per-frame recomputation. A segment whose recorded stream
  crosses up carries its own swap; the mirror re-latches at the same event.
- MK2's Block is a BUTTON (side-neutral), which simplifies this vs a
  hold-back-to-block family (asurabld); note the asymmetry, do not assume it
  ports.

This mechanism is the single biggest correctness risk and it is **directly
validated by the frame lab's replay classifier** (§8) before it ever fights.

---

## 6. Storage and retention

- Segments are an INDEX over recordings, so the recordings ARE the store; a
  `<model>/segments.jsonl` (or an sqlite index) holds the references +
  precomputed `start_features`, rebuilt cheaply from recordings.
- Per-matchup partitioning, exact→per-char→general fallback — the same
  fallback-KEY scheme as `shadow_runner`'s set loading, but a NEW loader and
  runtime: the existing one requires `cases.npz` and is fitted-kNN-shaped
  (rejects a `segments.jsonl` index). Pattern reuse, not code reuse (§10(b)
  is genuinely new Rust engine code).
- **Retention = KI's policy, directly reusable:** cap per matchup, recency
  wins, append-only as new matches arrive. KI kept up to 40 matches per
  opponent. Numbers are calibration.
- Always-on capture (§7 needs it) implies a disk budget — state it, cap it.

---

## 7. The organic arcade flow — a choreography state machine

Goal (user's words): start the app in shadow-mode and play arcade; once
enough samples are collected the shadow inserts a coin, presses P2 Start,
selects the character the user has been playing with the moves they've been
using; then the user picks any character and fights their shadow.

**Home: Lua for the choreography, Rust for the engine.** Which button is coin,
how the P2 select grid navigates, what "enough samples" means — all per-game
DATA/script (roadmap's per-game-training-scripts direction, CLAUDE.md's "per-
game knowledge is DATA" law). Segment storage/retrieval/mirroring is Rust,
once.

State machine:

```
ARCADE_RUN         — user plays 1P; recorder always on; segment index accretes
   │  (coverage: enough buckets filled — see below)
   ▼
CHALLENGER_READY   — threshold met; wait for a SAFE interrupt point [PENDING PROBE]
   │
   ▼
JOIN               — insert P2 coin, press P2 Start [PENDING PROBE: timing/screen]
   │
   ▼
SELECT_P2          — drive P2 cursor to the user's most-played char [PENDING PROBE:
   │                 P2 grid addresses, per-step timing, confirm + char_id verify]
   ▼
FIGHT              — hand P2 to the segment engine; user free-picks P1
```

**"Enough samples" is MEASURED, not chosen.** Key it on coverage buckets
actually filled, not a raw frame count — else the challenger shows up knowing
only how to walk forward. KI shipped 3 dojo matches as its baseline; our
analog is "every core bucket (neutral/offense/defense/oki/corner) has ≥ N".
**New code (review):** the `_bucket()` FUNCTION exists, but bucket counts are
computed only at FIT time into a model's meta.json, and matchup counts only
per me×opp cell — nothing counts bucket×matchup, and nothing counts anything
LIVE during a run. Since this engine's whole point is NO fit step, the
CHALLENGER_READY trigger needs a new online per-run per-bucket counter (Rust,
or a tailing process).

**"The character the user has been playing"** = the modal `me_char` across the
run's recordings (already read live). **"The moves they've been using"** is
FREE — a segment shadow can only replay what was recorded.

**A deliberate rules carve-out, written down:** SPEC §3c says Select/Start are
NEVER bot outputs. The choreographer MUST press them. This is an explicit
exemption: coin/Start/select injection is permitted ONLY in challenger mode,
ONLY on the choreographer's own state transitions, and must be impossible to
reach from the fighting engine.
**Open tension the probe resolves (review):** the draft said "only while the
gate is CLOSED (not in a fight)", but if the challenger interrupt is a
MID-FIGHT P2 Start (what `[PENDING PROBE]` is testing), the JOIN press happens
with the gate OPEN — the carve-out as first written would forbid the doc's own
mechanism. The gate-state condition on the carve-out is therefore **[PENDING
PROBE]**: it becomes "gate closed" if joins are between-round, or "gated to the
JOIN transition specifically" if joins are mid-fight. The principle (a narrow,
choreographer-only, unreachable-from-the-engine exemption) stands either way.

**PROBE RESOLVED (2026-09-01, `challenger-probe-evidence.md`)** — the flow is
FEASIBLE end-to-end; the measured facts:

- **JOIN is an IMMEDIATE mid-fight interrupt.** With P2 credits banked, a P2
  START mid-round freezes play in 2–6 frames, shows "PLAYER TWO HAS ENTERED",
  and reaches char-select ~120 frames (~2.2 s) later; the round is abandoned
  (no winner). So the carve-out's gate condition is settled: JOIN fires with
  the gate OPEN → the exemption is **gated to the JOIN transition
  specifically**, not "gate closed" (the §7 tension above, resolved this way).
- **SELECT_P2 is the normal dual-cursor select**, same grid/addresses as P1:
  `0xC1CA` (block2+0x0) is P2's live cursor, cursors independent. Full 4×3
  slot→char_id map measured. P2 default = Reptile (slot 3). Navigation is
  deterministic (edge-move frame 1, 12-frame auto-repeat; a 3f-tap/15f-gap
  moves exactly one cell, 7/7).
- **Two CONSTRAINTS the choreographer must handle:**
  1. **The select LOCK is flaky (~1-in-4 silent failure).** A silent miss
     ships the WRONG character. MANDATORY: verify the P2 cursor cell before
     lock, probe-tap lock, then re-verify `block2+0x0` AND `obj+0x3E` char-id
     AFTER the match loads — a renavigate-retry loop. This is the single
     biggest risk in the whole flow.
  2. **Credits: P2 needs its OWN 2 credits** (factory coinage; P1's were
     consumed at 1P start), there is NO RAM-readable mid-fight credit count,
     coins register only from SHORT frame-bounded presses (a held/wall-clock
     coin is debounced to nothing), and save-state loads rewrite the CMOS
     credit bank. An under-credited P2 START no-ops INVISIBLY — the
     choreographer must confirm the "PUSH START" screen state reached, not
     assume the coin took.
- **Both ports are input-live in the inherited fight** (differential
  walk-apart: P1-only −96/0, P2-only 0/+96, both −96/+96, zero cross-talk) —
  the state the engine inherits is sound.

Skips named by the probe (still open, lower-stakes): join timing in round 2 /
FINISH HIM / ladder / boss / CONTINUE screens; select-cursor edge wrap;
select-timer timeout; exact credit-debit moment; non-factory coinage.

---

## 8. Validation — before it fights anyone

The frame lab already owns the instrument: the **replay classifier**
(`frames.md §4.5`: ON-TIME / RETIMED / WHIFF / NO-EXECUTE / DIVERGED). Every
mechanism here is validated against it BEFORE deployment:

1. **Facing-mirror correctness (§5):** take a recorded segment, replay it
   through the OPPOSITE side, classify. **Reuse needs GLUE, not the classifier
   as-is (review):** its DIVERGED test compares executed inputs against the
   slot's OWN masks (`frames.md §4.5`), so a mirrored segment — which
   deliberately executes swapped masks — would be marked DIVERGED on every
   correct mirror unless the check runs against the MIRRORED expected stream;
   and ON-TIME/RETIMED exist only for replays that contain a contact, so a
   movement-only segment needs a separate fidelity check. The named glue:
   segment→InputSlot conversion, a mirrored expected-input reference, a
   per-segment origin run for the anchor, and a contactless-segment check. A
   correct mirror then reproduces the same moves (RETIMED at worst); a genuine
   DIVERGED is a mirror bug caught offline, never in a live fight.
2. **Divergence/stickiness (§4):** replay a segment into a deliberately
   perturbed state; confirm it breaks and re-selects at the intended
   deviation, not early (absorbing) or late (robotic).
3. **Fidelity (KI had no quantitative metric; we can do better):** held-out
   next-segment retrieval accuracy per bucket, and the A/B clip test (SPEC
   §7.4) — plus conditional action-frequency match per bucket, since players
   misjudge their own habits.
4. **The absorbing-state check** that bit the kNN shadow (dataset.py's
   neutral-cap note): a segment engine is structurally more resistant (it
   replays whole movement segments, so "stand still forever" is not
   reachable) — but VERIFY it, don't assume it. Name the control.

---

## 9. Open questions (resolve before/within implementation)

1. Overlap vs disjoint tiling (§2) — decide against §8.
2. Selection noise: KI's ~10%-band vs SPEC's temperature (§3.3) — A/B.
3. Trend-feature set and windows (§3.2) — start small, measure.
4. Similarity weights (§3.1): hand-authored seed, then tuned — by what
   objective? (per-bucket fidelity, §8.3).
5. Cross-matchup fallback fidelity (§6): KI's dojo-Jago "more or less works"
   against other characters — our fallback chain should surface WHICH tier
   answered (exact/per-char/general), not hide it.
6. Everything in §7 marked **[PENDING PROBE]**.
7. Does the recorder need a segment-boundary annotation column eventually
   (§1), or is an offline pass always enough?

## 10. Sequencing (proposed, for the plan that follows this doc)

Design review of THIS doc → fold in the challenger probe → then waves:
(a) segment index + similarity + retrieval, validated by §8.1/§8.2 offline;
(b) the segment engine as a deploy runtime alongside the kNN runner;
(c) the Lua choreographer + the coin-drop moment (§7), gated on the probe;
(d) the two KI wins the EXISTING kNN shadow can take — the SELF-trend feature
subset (§3.2, NOT the opponent-trends that MK2 can't compute) and weighted
retrieval (§3.1). **Not "cheap" (review):** changing `SCALAR_FEATURES`/X
breaks the G1 golden refit (`goat-v2` byte-identity, which `shadow_runner`
tests still read) and must move IN LOCKSTEP across both runtimes (`runtime.py`
and `src/shadow_runner.rs`, which rejects `feature_names` mismatches at load) —
the same byte-compat discipline MACRO_ACTIONS §4 imposes for asurabld. (d)
requires its own compat-gate plan, or it violates §0's "alongside, not
replace".

**Citations corrected from the draft:** temperature sampling is SPEC §3b (not
§8); SPEC has no live meter (that is an app surface, `frames.md §9`); §3.3
above should read SPEC §3b.
