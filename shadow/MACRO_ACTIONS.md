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
- Matcher tolerance: within a step, all of `press` must be down simultaneously
  for ≥1 frame while `dirs` are held (chord tolerance: presses may arrive up
  to 3 frames apart); between steps, at most `max_gap` = 12 frames. A macro
  with a single step is a chord (the slide); multi-step is a motion.

### Reference data this phase ships (Reptile, both MK2 ports)

- `slide`: **user-verified** — arcade `[{dirs:["back"], press:["LK","LP"]}]`,
  genesis `[{dirs:["back"], press:["LK","HK"]}]`.
- `acid_spit` (F,F,HP) and `force_ball` (B,B,HP+LP) are CANDIDATES: encode
  them, live-verify with the executor (projectile appears / hit_counter fires
  at range), and DROP any that fail verification rather than shipping guesses.

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
  the existing `hitstun_sources` mapping where present (asurabld), else a
  global `hit_counter`-change signal (MK2 arcade, verified `0xD3FE`;
  genesis pending — the RE task this wave). New optional profile key:
  `contact_signal: {"global": "hit_counter"}` — used when `hitstun_sources`
  is absent. No signal mapped → BlockPunish is offered greyed with a hint
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
