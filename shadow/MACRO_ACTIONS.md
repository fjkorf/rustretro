# MACRO ACTIONS — specials, the matcher/executor pair, and the block-punish dummy

Normative contract for the macro-action phase. Amends RECORDER_V3.md (row
annotation §3 below is v3-additive). Motivating cases: Reptile's slide is
`back+LK+HK` on Genesis but `back+LK+LP` on arcade (port-divergent encodings
— the arcade session's slides labeled as bare "LP" today); MK2 proximity
normals mean the same input is different MOVES by spacing.

Design law: **moves are not inputs.** A move is a named family-level intent;
an input encoding is port-level data; the game resolves variants (proximity)
at execution. One matcher recognizes encodings in input streams; one executor
plays them back; both read the same profile data.

## 1. Vocabulary — `family.json`

```jsonc
"moves": {
  "reptile": [
    { "name": "slide",      "tags": ["special", "low"] },
    { "name": "acid_spit",  "tags": ["special", "projectile"] },
    { "name": "force_ball", "tags": ["special", "projectile"] }
  ]
}
```

- Keyed by canonical roster NAME. A character absent from `moves` simply has
  no specials (asurabld stays untouched this phase — that is what keeps its
  goldens byte-identical).
- Tags are open strings; "special" marks label-space membership (§4).
- Proximity normals are NOT vocabulary this phase: the input is a base attack
  class and the game picks the variant. The frame-lab phase will add variant
  tables keyed by distance; recordings already carry both x's so variants are
  recoverable offline. (No state-id word exists on MK2 arcade — verified —
  so distance-at-press is the identification mechanism, contract-wide.)

## 2. Encodings — port profile `special_inputs`

```jsonc
"special_inputs": {
  "reptile": {
    "slide":      [ { "dirs": ["back"], "press": ["LK", "LP"], "frames": 4 } ],
    "acid_spit":  [ { "dirs": ["forward"], "frames": 3 },
                    { "dirs": ["forward"], "press": ["HP"], "frames": 3 } ]
  }
}
```

> **The example above is the PRE-correction draft, kept for schema shape
> only** (annotated 2026-09-01). Both entries were corrected by live audit:
> the arcade `slide` chord is `{dirs:["back"], press:["LK","LP","Block"],
> frames:8}` (Reference data below — without Block the chord resolves to a
> normal, and a 1-frame chord fires nothing, §11), and `acid_spit`'s trigger
> is a BARE `HP` step — `F · F · HP` — because a direction chorded with its
> trigger on the same frame does not register (§11; the `F,F+HP` shape shown
> here is retracted, see the Reference-data correction below).

- A macro is an ordered list of steps. Each step: optional `dirs` (held
  directions, SEMANTIC space: `back`/`forward`/`up`/`down` — side-resolved at
  match/execute time), optional `press` (attack-CLASS names from
  `attack_chords`, all pressed together), `frames` (hold length; default 3).
- **Omission is meaningful**: a port that lacks a move omits its entry, and
  every consumer offers only what the port encodes. Character key must exist
  in the family `moves` table; class names must exist in `attack_chords`
  (load-validated).
- Facing: `back`/`forward` resolve against live side = sign(opp.x − me.x)
  (fighter field `facing` MAY refine this when mapped). Both matcher and
  executor MUST use the same resolution.
- Matcher semantics: a step is satisfied at frame `i` when `dirs` are held at
  `i` and every `press` class's full chord is down AT `i` — simultaneously,
  in that one frame; the game reads button state per frame, so simultaneity
  is the rule, not a trailing "recently pressed" window. A macro completes on
  the rising edge of its final step's satisfaction (satisfied now, not
  satisfied the frame before) — one input is one move, so a chord held for
  many frames fires once, not once per frame; it re-arms only once that final
  step releases. Between steps, at most `max_gap` = 12 frames (unchanged). A
  macro with a single step is a chord (the slide); multi-step is a motion.

### Reference data this phase ships (Reptile, both MK2 ports)

- `slide`: LIVE-VERIFIED (correcting the user-reported chord for arcade):
  arcade `[{dirs:["back"], press:["LK","LP","Block"], frames:8}]` — without
  Block the chord resolves to a normal, and point-blank it resolves to a
  close normal/throw (the §1 proximity rule; a point-blank punish slide
  whiffs by game rule); genesis `[{dirs:["back"], press:["LK","HK"]}]`.
- `acid_spit` (F,F,HP) and `force_ball` (B,B,HP+LP) are CANDIDATES: encode
  them, live-verify with the executor (projectile appears / hit_counter fires
  at range), and DROP any that fail verification rather than shipping guesses.

  > **Since resolved (2026-08-30 audit; correction recorded 2026-09-01).**
  > `force_ball` is VERIFIED as `B · B+HP+LP` — the chorded back is the
  > second tap of the motion, not a held-direction chord (§11). `acid_spit`'s
  > first "verification" (h1 −24 via the punish executor) was **RETRACTED**:
  > the trial ran at point blank and 24 is exactly Reptile's *close HP
  > normal*; the chorded `F · F+HP` form produces no special at all (0
  > damage, 16/16 configurations). The shipped encoding is `F` · `F` · `HP`
  > — bare trigger tap (§11; evidence in mk2.md "Special-move encodings,
  > live-audited"). And "hit_counter fires at range" was never a valid
  > verification signal either — `0xD3FE` is P1-victim-only (§6's own
  > amendment); verify a projectile by damage at a gap where every normal
  > provably whiffs.

## 3. Recorder annotation (RECORDER_V3 v3-additive)

When the live matcher completes a special for a player, that frame's row
gains `"p1_special":"slide"` (or `p2_special`), appended AFTER `p2_input` in
the fixed key order; absent otherwise. The rounds sidecar gains
`"specials": {"slide": 3, ...}` counts (union of both players) per round.
Rows stay raw otherwise; old readers ignore unknown keys. The recorder's
matcher instance uses the recording profile's encodings; ports with no
`special_inputs` incur zero overhead.

## 4. Label space (Python, `shadow_train`)

- Attack-head classes = family `attack_classes` + sorted names of every
  family move tagged "special" (family-level and port-independent, so
  cross-port models share one head; a port that can't perform a special just
  never emits or executes that label).
- Decision labeling: the TRAIN-SIDE matcher is authoritative (old recordings
  without annotations still label correctly); a special completing within a
  decision window overrides the base attack class for that decision.
  Annotations (§3) are for humans/coverage, not the labeler.
- asurabld: no `moves` table → label space unchanged → G1/G2 gates must stay
  byte-identical. This is the phase's hard back-compat gate.
- `report`/`coverage` surface per-special usage counts.

## 5. The executor (Rust)

`MacroExec`: given (macro, port, side-resolution source), emits a 12-bit mask
per frame until done — dirs resolved to left/right per facing, `press`
classes compiled through the ACTIVE profile's `attack_chords` (this is what
makes a cross-port ghost's `slide` intent press the right buttons on each
port). Injection uses the existing per-port injected-input path (2-frame
hold idiom). Shared by the dummy (§6) now and the shadow runner's special
intents later (NOT this phase — runner executes only base classes until the
label space ships in models).

## 6. Block-punish dummy (`DummyMode::BlockPunish`)

- Dummy guards per family block style (button → hold the Block chord;
  back_hold → hold away, existing logic).
- **Trigger** = dummy is guarding AND the profile's contact signal fires:
  the existing `hitstun_sources` mapping where present, else the
  `contact_signal` global. AMENDED by live findings: arcade `hit_counter`
  0xD3FE is P1-victim-only (never fires for hits ON P2) — DROPPED as the
  arcade trigger; arcade uses `hitstun_sources` = the per-player HUD damage
  pair (caveat: training refill rewrites those bytes — one spurious punish
  per refill, absorbed by the cooldown). **Superseded in turn (2026-09-01):
  the arcade trigger is now the shipped `contact_signal {"field": "health",
  "direction": "decrease"}` — STRUCT health `block+0x0E`, which steps by the
  whole damage in one frame (the HUD pair is a DRAWN value that smears one
  hit into ~11 edges), with `direction: "decrease"` making the trigger
  immune to both known increase hazards (the round-intro health ramp and the
  training refill — closing the spurious-punish caveat above, inverted into
  ~one LOST trigger per refill cycle). Blocked normals chip 3/6/8 on this
  port, so a health-delta trigger sees blocked contact. `hitstun_sources`
  stays as the fallback. Evidence: mk2.md "Contact signal shipped"
  (2026-09-01).** Genesis has NO verified contact
  signal (honest negative — VFX-cluster candidate false-fires on movement);
  BlockPunish greys there until one lands. No signal mapped → BlockPunish is offered greyed with a hint
  (per-feature degradation, house pattern).
- **On trigger, select from a WEIGHTED OPTION POOL** (never deterministic —
  the survey's number-one finding): options are `{move: <special name>}`,
  `{attack: <base class>}`, `{throw: true}` (deferred until throw RE),
  `{continue_block: N frames}`. Panel default pool: selected move w=3,
  continue-block w=1. Cooldown: no re-trigger until the contact signal has
  been quiet ≥ HITSTUN_RECENT_FRAMES.
- Character-aware: dummy's char_id → canonical → family `moves` ∩ port
  `special_inputs` + base attack classes = the legal option list the panel
  shows (with weight steppers).

## 7. W1 ownership

- **A-Rust (fable)**: `src/macros.rs` (types, matcher, executor, tests),
  `src/training.rs` (BlockPunish mode), `src/record.rs` (annotation +
  x-liveness warning: after 300 controllable frames with zero x variance,
  log "x looks frozen — object slot may have moved (mk2.md)"),
  `src/debug/panels/training.rs` (option-pool UI), `src/profile.rs`
  (moves/special_inputs/contact_signal schema + validation),
  `library/mk2/family.json` + `library/mk2/mk2.profile.json` (reference
  data §2 + arcade contact_signal).
- **A-Python (sonnet)**: `shadow_train` matcher mirror + label space + report
  counts + tests (incl. the asurabld byte-compat gate and a synthetic
  slide-labeling fixture); `re.py` docstring notes from the field-test
  friction (poll-rate guidance, press timing).
- **A-RE (sonnet)**: genesis `hit_counter` hunt (`library/mk2/genesis.profile.json`
  + `mk2-genesis.md` ONLY — paste the genesis slide encoding from §2 while
  in the file); recapture a real, input-live arcade arena over
  `shadow/arenas/mk2/reptile-vs-reptile.state` (the committed one is an
  attract demo — coin/start flow is documented in mk2.md).
- File conflicts resolved by the split above; the contract's §2 JSON is
  pasted verbatim by whichever agent owns each file.

## 8. W2 scope (added after the juggle/wakeup design review)

State-free tracking, per the review's findings (no state word needed):

1. **String segmentation stats** in the rounds sidecar: contact events come
   from the port's contact signal ~~(arcade `hit_counter` — its own ~20-frame
   reset window IS the game's linking judgment: consecutive increments
   without reset = one string; genesis analog pending A-RE)~~. Each event
   classified hit-vs-block ~~by defender health delta (MK2: blocked normals
   do 0, blocked specials chip)~~. Per round: string count, longest string
   (hits + damage), block-string count. Feeds report/coverage as drill
   inputs ("juggle conversion after launcher: 2/9").

   > **CORRECTED 2026-09-01 — both struck clauses were written against
   > evidence the docs had already retracted when this section was drafted.**
   > (a) Arcade `hit_counter` 0xD3FE is P1-victim-only (§6's own amendment)
   > and must never source string events. The arcade contact source is the
   > per-victim health signal — now the shipped
   > `contact_signal {"field": "health", "direction": "decrease"}` on STRUCT
   > health `block+0x0E` (§6 above; mk2.md "Contact signal shipped",
   > 2026-09-01) — with the ~20-frame quiet window
   > (`HITSTUN_RECENT_FRAMES`) supplying the linking judgment instead of the
   > counter's reset. The genesis analog is still an honest negative (§6).
   > (b) "MK2: blocked normals do 0" is measured FALSE: blocked normals CHIP
   > on arcade struct health — 3/6/8, exactly a quarter of the damage
   > (docs/frames.md §4.1's chip note; mk2.md's ladder tables) — so
   > hit-vs-block cannot be classified by a zero health delta at all: zero
   > damage means WHIFF, not block (frames.md §2.6). Classify hit-vs-block
   > from the rig's own guard state where the rig drives the defender, else
   > from the chip-vs-full damage signature.
2. **Juggle context flag**: genesis = defender y off ground at contact;
   arcade = the `+0xC` latch (0xFFFC ~366ms, fires on launcher hits) —
   VERIFY the latch holds through the juggle and clears on landing before
   trusting it (RE nibble, assigned to A-RE's arcade visit).
3. **Live string/juggle ticker overlay** (optional polish): hits-in-string,
   string damage, JUGGLE flag — gui.text or native.
4. **Explicitly deferred to the frame lab**: all wakeup timing. Pattern of
   record: event detection + measured per-move duration tables (probe-poke
   protocol) replaces live state detection; the tables are the frame-data
   DB's first contents.

## 9. Guard policy — back-to-block families (added after the back-hold research pass)

MK2's dummy guards with a BUTTON, which is positionally inert. Asura Blade
(and SF-style families) block by HOLDING AWAY, which is not: measured live,
a continuously-guarding asurabld dummy widened the gap 165 → 286 units in
~1 s and then sat cornered. Three further live results shape this section:
holding away DOES block (0 dmg vs 42 standing, 3/3); blocked attacks deal
ZERO chip; and the combo counters (`hitstun_sources`) do NOT fire on blocked
contact (3/3 quiet). A full-struct diff found 8 bytes that fire on blocked
contact but ALL of them also fire on whiffs — because a back-holding dummy
is walking, so its struct never sits still. Contact detection and spacing
fail for the same root cause.

### 9.1 `guard_policy` (family-level, `family.json` `block`)

- `"style": "button"` → today's behaviour: hold the block chord. Inert.
- `"style": "back_hold"` → **reactive** guard. The dummy stands NEUTRAL and
  asserts away-direction ONLY inside a guard window. This is the pattern
  every surveyed trainer uses (peon2/fbneo-training-mode `AutoBlock`, which
  releases the direction the moment the attacker is neutral, and gates on
  distance so it never reacts to far whiffs). Holding unconditionally is
  forbidden — it destroys spacing within a second.

### 9.2 The guard window

Open when BOTH hold: (a) the opponent is committing an attack, and (b) the
opponent is within `guard_range` (port profile, units of the mapped `x`).
Close otherwise, and release the direction fully.

Attack-commitment source, in preference order:
1. **The opponent's live input mask** — we already capture both ports, so
   this needs ZERO new RE, is frame-exact, and works on any family. Hold for
   `guard_hold_frames` after the press to cover startup + active.
2. Attacker `action`/`anim` state values (asurabld maps both) — a refinement
   that also covers CPU opponents and ends the window when the animation
   does. Requires a per-port list of attacking action values.
Ship (1); (2) is an upgrade, not a prerequisite.

### 9.3 Punish trigger for back_hold families

Blocked contact is UNDETECTABLE on asurabld today (zero chip, quiet
counters, and the defender's struct churns while walking). So the trigger is
**"the opponent committed an attack inside `guard_range`"** — the same event
that opened the guard window. This is a SUPERSET drill (block-punish AND
whiff-punish) and is honest about what it detects: the panel/phase must say
"punishing (commit)" rather than implying blocked contact was confirmed.
Distinguishing blocked from whiffed is deferred; the most promising lead is
pushback (a blocked hit shoves the defender, and a reactive dummy is static
enough for that to read).

### 9.4 Guard modes (the vocabulary players expect)

`Guard All` / `After First Hit` / `Random (weighted)` / `None` — matching
SF6, GGST, and fbneo-training-mode's own selector. `After First Hit` needs a
hit signal, which asurabld HAS (the combo counters fire on hits, just not on
blocks), so it is implementable there.

### 9.5 Hazards to check before shipping asurabld

- **Charge moves**: holding back genuinely accumulates charge (confirmed —
  fbneo-training-mode pokes charge-timer bytes precisely because of this).
  If any asurabld character is charge-based, a guard hold silently arms
  their specials. Reactive guard mostly avoids this by construction; verify.
- **Lows vs overheads**: down-back blocks everything in SF2-era engines, so
  those trainers just add `Down` for grounded attacks. Unverified for
  asurabld — check whether true overheads exist before choosing the rule.
- **No guard-state byte**: no surveyed trainer found one; blocking is always
  driven by real input. Do NOT spend RE time hunting for that shortcut.

### 9.6 Third-party cross-validation

`peon2/fbneo-training-mode`'s `games/asurabld/asurabld.lua` carries
independently-derived addresses; its combo counters (`0x4041E7`/`0x40470B`)
MATCH ours exactly. Cross-check facing (`0x4037F9`/`0x4045AD`), health,
meter, and character-id against our profile — free verification. It
implements no AutoBlock for this game; we are first.

## 10. Step vocabulary extensions (added after the MK2 Mileena audit)

The published MK2 arcade movelists break two assumptions baked into §2. Both
are contract amendments, not data gaps — the current DSL cannot express
these moves at all.

### 10.1 Release-triggered and charged steps

Mileena's Sai Throw is **hold HP for ~3 seconds, then release**. Reptile's
Invisibility is **hold BLK across a whole directional sequence, then release
and press HP**. §2's matcher fires on the RISING EDGE with re-arm on
release, so a move whose defining moment is a RELEASE is invisible to it.

Two new step kinds, both expressible in the existing ordered-step model:

- `{"hold": ["HP"], "min_frames": 150}` — the step is satisfied while the
  chord stays down, and is only COMPLETE once it has been held at least
  `min_frames`. A release before `min_frames` fails the macro.
- `{"release": ["HP"]}` — satisfied on the FALLING edge of that chord.

Consequences that must be honored by both the Rust matcher and the Python
twin, or the two halves diverge:

- Completion for a macro whose final step is a `release` fires on the
  falling edge, not the rising one. The "rising edge with re-arm on release"
  rule in §2 becomes: **completion fires on the edge the FINAL step names.**
- A `hold` step spanning other steps (Reptile's `[BLK] U U D HP`) means
  chords are no longer strictly sequential. Model it as a step-scoped
  `while_held` chord on the intervening steps rather than inventing nesting.
- Charge accumulation is a live hazard, not just a matching concern
  (§9.5): a dummy or executor that parks on a button silently arms these
  moves. The executor MUST release held chords when a macro aborts.

### 10.2 Side-swapping moves

> **CORRECTED 2026-08-30 by live audit: Teleport Kick does NOT swap sides.**
> Measured 4/4 (at 192 px and after 30/50/65-frame walk-ins), Mileena ends
> on the same side — she teleports **vertically**, `y` swinging from +200
> (underground) to −131 (above the screen) with `x` discontinuous
> (945→1013 in one frame). This section was built around the wrong move.
> The confirmed side-swapper is the **ROLL** (3/3): it travels through the
> opponent, 915→1192 while pushing him 1119→1002. The facing-pin rule below
> is correct and still wanted — it is simply the roll that needs it. The
> `side_swap` tag stays on teleport_kick because the pin is conservative,
> flagged as unconfirmed rather than removed.

Mileena's Teleport Kick crosses the opponent mid-move. §2 resolves
`back`/`forward` against live facing, which is correct frame by frame and
therefore WRONG across a swap: the same macro's later steps resolve against
the flipped side.

- **Matcher**: semantic directions resolve against the facing at the frame
  the macro STARTED, pinned for the macro's duration. Otherwise a teleport
  retroactively invalidates its own inputs.
- **Executor**: same pin, for the same reason.
- **Downstream**: gap-keyed data (frames.md §5) is discontinuous across a
  swap. A side change between anchor and probe invalidates that measurement;
  the harness must detect the facing flip and discard, not record.


## 11. The direction/press timing rule (live-audited 2026-08-30)

**A direction chorded with its trigger button ON THE SAME FRAME does not
register on MK2 arcade.** The direction must be down at least one frame
BEFORE the press, and need not still be held at the press.

Measured: `F · F+HP` produced 0 damage in **all 16** frames×gap
configurations; `F · F · HP` produced 15 damage in every config at an
inter-step gap ≥ 2. This invalidates the chorded shape for every motion
special on this port.

> **BOTH "exemptions" recorded here were WRONG, and dissolving them produced
> the real rule (Baraka audit, 2026-08-30).**

**The real rule: a chorded special needs the direction and the button down
TOGETHER for ≥2 consecutive frames.** It is the CONJUNCTION that needs two
frames, not the direction — back held 5 frames then HP for 1 still fails —
and the trigger is not the button's rising edge: pressing HP one frame
*before* back and then overlapping for 2 frames fires it.

- **"Single-frame chords are exempt" was false.** There is no single-frame
  chord on this port. Baraka's Blade Swipe: 1 frame → 0 damage, 2–12 frames
  → 32. Reproduced independently on **Reptile's slide** (1 frame → 0, 2–12
  → 13) — it has always shipped `frames: 8`, so nobody had ever varied it
  and the "exemption" was an artifact of never testing the boundary.
- **`force_ball` was never an exemption either.** Its chord's `back` is the
  second tap of a `B,B` motion, not a chorded direction: `B · HP+LP` = 0,
  `B+HP+LP` alone = 0 at every hold length, `B · F+HP+LP` = 0.

**Still open, stated as a hypothesis rather than a rule**: which motions
accept a chorded FINAL tap. `force_ball`, `teleport_kick` and
`blade_shredder` do; `acid_spit`, `roll` and `blade_spark` do not. The only
predicate fitting all six is "the trigger contains LP or LK" — 6/6, no
mechanism, and the falsifying test is a chorded final tap on a
punch-triggered motion.

**Executor constraint**: the inter-step gap must be ≥ 2 frames for
**repeated-direction** motions, and `STEP_GAP = 2` sits exactly on that
boundary. Corrected: this does NOT hold universally — `blade_spark`
(`D · B · HP`, two DIFFERENT directions) fires at neutral gap 0 and 1.

## 12. Timing bounds the DSL cannot express (Baraka audit, 2026-08-30)

Every step interval in this DSL is currently UNBOUNDED, which is wrong in
both directions: real moves fail if you are too slow, and a matcher with no
upper bound will annotate a special in a recording that no human performed.

Measured on MK2 arcade, each by bisection from both ends:

| bound | evidence |
|---|---|
| **onset-to-onset window** | `double_kick`'s second press must land 11–16 frames after the first press's ONSET (10 fails 3/3, 11 fires 3/3, 16 fires, 17 fails) — and it is INVARIANT under the first press's hold length (1/2/3/5/8), so onset-to-onset is the rule and any neutral-gap framing is an executor artifact |
| **whole-macro span cap** | `blade_spark` fires at span ≤16 and fails ≥17; `[9,9]`, `[13,7]`, `[7,13]` all fail despite every per-step gap being legal |
| **both at once** | `blade_shredder` needs a per-step cap (dir taps ≤13 apart) AND a span cap (≤34): `[14,14,3]` (span 31) fails on the first, `[3,3,29]` (all gaps legal) fails on the second |
| **proximity precondition** | `double_kick` is available at ≤64 px and unavailable at ≥65, and that boundary is EXACTLY the game's close/far HK variant switch (16 dmg @f11 → 32 dmg @f8). "Close" is §1's proximity-normal selection, not a separate gate — so do NOT add a close/far boolean |

Note the precondition cannot be inferred at runtime: `block+0xC0` fires
twice in every `HK HK` trial at every range, so "came out and whiffed" and
"did not come out" are indistinguishable from the action counter. Only the
two-hit damage signature separates them, which is why the gap precondition
has to live in the DSL rather than be detected.

**Measurement hazard — never audit a move at its reach boundary.** Blade
Swipe at 117 px depends on the DEFENDER'S IDLE-ANIMATION PHASE (settle 0
fires, 1–8 miss, 9–11 fire), and the phase reference DRIFTS across a
session: a batch that verified the move there stopped reproducing an hour
later from the same save state with byte-identical positions. Anchor every
verdict well inside the connect region.
