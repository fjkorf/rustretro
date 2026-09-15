# MK2 combat demo — verification report

What every beat in `shadow/demos/mk2/*.beats.json` CLAIMED, what the emulator
actually did, and the control run that makes each claim differential rather
than anecdotal. Nothing here is a number copied out of
`library/mk2/arcade.frames.json`: every row was produced by driving both ports
on a headless instance (port 4026, `fbneo_libretro.dylib`, `mk2.zip`) and
reading the struct-health damage register (`block + 0x0E`) and the
object-pointer `x`/`y`. Nothing is read off the drawn HUD bar
(docs/frames.md §4.1 — it animates 1 unit/frame).

**Stage.** All five sequences are Mileena (P1, char id 5) vs Reptile (P2, char
id 9) on the `shadow/arenas/mk2/m-gap-*.state` ladder. Every arena is loaded
through `LabSession.load_state` (atomic `pause_after` + identity/liveness
verifier, docs/frames.md §3.4/§4.6); `press_buttons` is never used;
`enable_writes` and training-enforcement-off are enforced per session.

**Determinism.** Every sequence was executed 3× from its arena; the full
(frame, damage) signature of BOTH ports was identical on all 3 runs of all 5
sequences. Every control below was also run 3× (except the 5-rung teleport
sweep, 1× per rung) and was identical across reps.

**Slot cross-check.** Each sequence was re-executed stepped with
`record_inputs` running; the recorder's own slot was byte-compared against the
exported `shadow/inputs/mk2/demo-<n>-<slug>.slot.json`. All 5: identical frame
lists, identical lengths, `lock_skips.training` +0 and
`lock_skips.training_during_playback` +0 across the run.

**Slot replay.** Each slot was additionally replayed through
`play_inputs(port="both", trigger="manual")` from its own arena, stepped. All
five reproduced their authored fight exactly, with a UNIFORM +1-frame offset
on both ports (e.g. sequence 1's contacts f11/f30/f55 → f12/f31/f56). The
offset is the playback arming frame, it is the same on both ports, and it
changes no interaction. (The brief predicted +2; measured here it is +1 with a
sample-then-step loop. Reported as measured.)

---

## Reading the damage column

MK2 arcade chips on block, so **zero damage means WHIFF, not block**
(docs/frames.md §2.6). Every outcome below is classified by the damage
REGISTER, never by "did damage happen":

| move | full hit | blocked (chip) |
|---|---|---|
| Mileena far HP | 11 | 3 |
| Mileena far HK | 32 | 8 |
| Mileena close HK | 16 | 4 |
| Mileena cLK | 6 | 2 |
| Mileena roll | 21 | 5 |
| Mileena teleport kick | 32 | 4 |
| Reptile far HK | 32 | 8 |
| Reptile close HK | 16 | 4 |
| Reptile far HP | 11 | 3 |
| Reptile close HP | 24 | 6 |
| Reptile slide | 13 | 3 |

---

## 1 — "Jabs don't pass the turn" (`m-gap-39`, gap 71 px)

Claim: far HP is **+13 on block**, so a blocked jab does not return the turn.

| beat | span | expected | observed | verdict |
|---|---|---|---|---|
| `jab-1-blocked` | [0,19) | contact on P2, −3 chip, P2 guarding | P2 161→158 at **f11**; P2 held Block from f0 | PASS |
| `jab-2-blocked` | [19,38) | contact on P2, −3 chip, P2 still guarding | P2 158→155 at **f30**; jab pressed at f19 = her own earliest free frame | PASS |
| `jab-3-stuffs-the-counter` | [38,98) | P2 −11 (a HIT), P1 −0 | P2 155→**144** at **f55**; P1 161→161 for the whole beat | PASS |

**The number, measured independently.** Press-sweep rig (sweep the frame each
port's next attack button is asserted; the earliest frame that produces
contact is that port's earliest actionable frame):

- Mileena's earliest re-press after her own blocked far HP: **f19** (f15–f18
  produce nothing; f19 → contact f30, f20 → f31, …).
- Reptile's earliest press, guard released at contact+1: **f32** (f30, f31
  dead; f32 → contact f43, f33 → f44, …).
- **32 − 19 = +13.** Exactly `arcade.frames.json`'s `mileena/HP/far/gap_px 71
  → on_block +13`, from a rig that is not the act-again walk probe that
  produced it.

**Controls (3 reps each, identical).**

| control | one variable | result |
|---|---|---|
| 1-C1: identical defender program, **no jab 3** | Mileena's third jab removed | Reptile's HK comes out and lands **32 on Mileena at f59** — his input was valid, in range, and correctly timed |
| 1-C2: jab 3 with Reptile **still guarding** | his guard | jab 3 chips **3**, not 11 — the 11 in the beat is a real HIT |
| 1-C3 / 1-C4: Reptile presses at f32 / f46 under jab-3 pressure | his press frame | both dead: P1 loses 0, P2 still eats the f55 hit for 11 |

So the beat's 11 damage is not choreography luck: the same counter that lands
32 without jab 3 lands nothing with it, and Reptile never gets a frame.

**Side findings recorded here, not used by the beats.**

- A **chain window**: a second far HP pressed at f12/f13/f14 comes out
  IMMEDIATELY (contact f23/f24/f25) — 7 frames before her own earliest free
  re-press. f15–f18 produce nothing at all. The sequence deliberately uses
  f19, because f19 is the frame the +13 number is about.
- Blocked far HP pushback is **+9 px** per jab (71 → 80 → 89), so the third
  jab needs the 5-frame step-in to stay inside far HP's 83 px connect range.
- Releasing Block **1 frame before** contact takes a full hit; releasing **on**
  the contact frame still blocks (swept f45…f50 against a f50 contact).

---

## 2 — "The hit-confirm" (`m-gap-39`, gap 71 px)

Claim: far HK is **+3 on hit / −20 on block**. Same button, same frame, same
spacing; the only variable is whether Reptile held Block.

| beat | span | expected | observed | verdict |
|---|---|---|---|---|
| `far-HK-hits` | [0,20) | P2 −32, P2 not guarding | P2 161→**129** at **f8**; gap 71 → 154 | PASS |
| `no-punish-on-hit` | [20,45) | P1 −0 | the identical counter program (HK at contact+29) reaches nothing; gap at the press is **149 px** vs Reptile's 110 px connect range | PASS |
| `she-keeps-the-turn` | [45,96) | P2 −11 | she walks 154 px → 75 px in 30 frames and lands far HP: P2 129→**118** at **f88** | PASS |
| `same-button-blocked` | [96,125) | P2 −8 chip, P2 guarding | P2 118→**110** at **f104** | PASS |
| `the-turn-transfers` | [125,150) | P1 −32, P1 guarding, punish connects | P1 161→**129** at **f133**, with Mileena holding Block since f99 | PASS |

**The number, measured independently.** Same press-sweep rig on the blocked
far HK:

- Reptile (defender) earliest press, guard released at contact+1: **f29**
  (f24–f28 dead; f29 → contact f37).
- Mileena (attacker) earliest re-press: **f49** (f48 dead; f49 → contact f57).
- **29 − 49 = −20.** Exactly `mileena/HK/far → on_block −20`.

**Controls (3 reps each, identical).**

| control | one variable | result |
|---|---|---|
| 2-C1: blocked far HK, punish at f29, **Mileena not guarding** | her guard | P1 loses **32** at f37 |
| 2-C2: identical, **Mileena guarding from f3** | her guard | P1 loses **32** at f37 — unchanged. Her guard has not returned, so this is a PUNISH, not a hit she chose to eat |
| 2-C3: far HK **HIT**, identical counter program | Reptile's Block during her HK | P1 loses **0** |

C1/C2/C3 are the whole thesis in three rows: the counter is the same, the
timing is the same, and only "did he block it" decides whether it collects 32.

**Finding — the brief's close-HP punisher is impossible here.** A blocked far
HK adds **+24 px** of pushback (71 → 95). Reptile's HP has a 72 px connect
range and this ladder's floor is 61 px, so no rung leaves him inside HP range
after blocking it. The punisher that works is his far HK (110 px). This is
docs/frames.md §1's range clause, and it is why the beats use HK.

**Refused.** Reptile's far HK **on hit** could not be re-verified against a
Mileena defender with this rig: the hit knocks the victim to a 150 px gap and
Mileena's longest connect range is 114 px, so she has no move to answer with.
Named reason: RANGE. The mirror-measured `+7` is left uncorroborated rather
than reused as if confirmed.

---

## 3 — "Over-commit and pay" (`m-gap-0`, gap 192 px)

Claim: the roll is −34 and pays; the teleport kick is −25 and **does not**,
because of range.

| beat | span | expected | observed | verdict |
|---|---|---|---|---|
| `roll-blocked` | [0,34) | P2 −5 chip, P2 guarding | P2 161→**156** at **f33** | PASS |
| `roll-punished` | [34,90) | P1 −32, P1 guarding, punish connects | P1 161→**129** at **f69**, Mileena holding Block since f19 | PASS |
| `teleport-blocked` | [90,133) | P2 −4 chip, P2 guarding | P2 156→**152** at **f132** | PASS |
| `punish-refused-by-range` | [133,190) | P1 −0 | the same counter that collected 32 off the roll reaches nothing; gap 153 px | PASS |

**Controls.**

| control | one variable | result |
|---|---|---|
| 3-C1: roll punish, **Mileena guarding from f19** | her guard | P1 loses **32** at f69 |
| 3-C2: roll punish, **Mileena not guarding** | her guard | P1 loses **32** at f69 — identical, so the guard was irrelevant: true punish |
| 3-C3: blocked teleport from 5 further rungs (146/114/99/83/71 px) | the starting gap | **every one** ends at a **153 px** gap, contact always f37, chip always 4 |

### REFUSED: "blocked teleport_kick → punished again"

Named reason: **RANGE**. The blocked teleport kick lands Mileena at a FIXED
153 px gap from every starting rung measured (192/146/114/99/83/71 px — 6
rungs, 1 rep each, all identical), and no Reptile row in
`arcade.frames.json` has a `connect_range` above 110 (far HK / far LK). A −25
move that is unpunishable at every distance it can be thrown from is
docs/frames.md §1's third clause in its purest form, so the beat is kept as an
executed NON-punish rather than dropped — the refusal is the lesson.

### FINDING: the roll's on-block number differs between rigs

Press-sweep rig on the blocked roll at `m-gap-0`:

- Reptile (defender) earliest press, guard released at contact+1: **f56**
  (f40–f55 dead).
- Mileena (attacker) earliest re-press (far LK, the only button of hers with
  enough range at the post-roll 103 px gap): **f84** (f64–f83 dead).
- **56 − 84 = −28**, against `arcade.frames.json`'s **−34**.

This is NOT published as a correction. The two rigs measure different
predicates — the table's act-again probe asks "can the fighter start a WALK",
this one asks "can the fighter start an ATTACK" — and docs/frames.md §1
already records that guard (and, here, attack) returns before the walk does,
which predicts exactly this sign of disagreement. The same press rig
reproduced +13, −20, −2 and −14 EXACTLY on four other rows, so a systematic
offset is not the obvious explanation either. What is recorded is that the
lab's two rigs disagree by 6 frames on the most negative row in the MK2 table,
and that re-measuring the roll with the table's own probe is the way to settle
it. The demo's punish claim does not rest on the integer: it rests on C1/C2,
where the counter lands 32 whether or not Mileena is guarding.

---

## 4 — "Safe means unpunishable" (`m-gap-45`, gap 61 px)

Claim: `punishable ⟺ advantage ≤ −(punisher's first_active_frame)`. One
punisher, one program, two moves.

| beat | span | expected | observed | verdict |
|---|---|---|---|---|
| `safe-move-blocked` | [0,21) | P2 −2 chip, P2 guarding | P2 161→**159** at **f20** (cLK, 10-frame crouch lead-in) | PASS |
| `fastest-punish-fails` | [21,66) | P1 −8, i.e. CHIP | P1 161→**153** at **f45** — Mileena blocked it | PASS |
| `reset-to-point-blank` | [66,94) | no damage either side | none; gap 117 → 63 px | PASS (staging, not a claim) |
| `unsafe-move-blocked` | [94,122) | P2 −4 chip, P2 guarding | P2 159→**155** at **f105** (close HK: 4 chip, so gap 63 px is inside the close variant) | PASS |
| `the-punish-lands` | [122,160) | P1 −16 FULL, P1 guarding, punish connects | P1 153→**137** at **f133** with Mileena holding Block since f107 | PASS |

**Both numbers, measured independently.**

| move | defender's earliest press | attacker's earliest re-press | difference | table |
|---|---|---|---|---|
| Mileena cLK, blocked | **f37** (f24–f36 dead) | **f39** (f24–f38 dead) | **−2** | −2 ✓ |
| Mileena close HK, blocked | **f28** (f14–f27 dead) | **f42** (f38–f41 dead) | **−14** | −14 ✓ |

Reptile's fastest button reaches on frame 8 (measured at this rung: press → 24
damage at press+8, his close HP). −2 > −8 → not punishable. −14 ≤ −8 →
punishable. The beats are that arithmetic executed.

**Controls (3 reps each, identical) — the differential.**

| control | one variable | result |
|---|---|---|
| 4-C1: cLK, counter HK at f37, **Mileena NOT guarding** | her guard | P1 loses **32** — the counter is real, in range, and lethal if she cannot get guard back |
| 4-C2: cLK, counter HK at f37, **Mileena guards at f39** | her guard | P1 loses **8** (chip). She was actionable in time; the move is safe |
| 4-C4: close HK, counter at f28, **Mileena guards ASAP at f13** | nothing — she guards as early as the input allows | P1 loses **16 FULL**. Her guard cannot come back: a true punish |
| 4-C5: close HK, counter at f28, **Mileena not guarding** | her guard | P1 loses **16** — identical to C4, confirming the guard was never available |

C2 vs C4 is the one-variable pair the sequence is built on: the same counter,
the same guard-release discipline, the same earliest-actionable timing — only
the move changes, and only the −14 one is punishable.

**Finding — why the punisher is HK and not close HP.** Reptile's close HP is
the fastest thing he owns (contact at press+8, 24 damage, measured at gap 61
on this ladder). It is still the wrong button here, because cLK's blocked
pushback is **+32 px** (61 → 93) and his HP's connect range is 72 px:

- 4-C3: cLK, counter **close HP** at f37 → **no contact at all**.

Using HP in one half and HK in the other would have made the comparison
two-variable. Both halves use HK so that the only difference between them is
the move being punished. Recorded because it is the second time in this demo
that a published negative number is neutralised by pushback.

---

## 5 — "Knockdown currency" (`m-gap-25`, gap 114 px)

Claim: a knockdown has no on-hit advantage integer (docs/frames.md §1.1); what
it buys is measurable, and it is an approach, not a meaty.

| beat | span | expected | observed | verdict |
|---|---|---|---|---|
| `slide-hits-knockdown` | [0,45) | P1 −13, P1 not guarding | P1 161→**148** at **f11**; victim's own `y` leaves its resting 87 for f12–f41 (apex 42) and returns at f42 | PASS |
| `the-wakeup-clock` | [45,86) | no damage either side | none; Reptile closes 160 px → 77 px | PASS |
| `jab-pressure-resumes` | [86,130) | P1 −6 total, P1 guarding | P1 148→**145** at **f97** and 145→**142** at **f116** — two blocked far HPs (3 chip each; 11 would be a hit), 19 frames apart | PASS |

**The wakeup clock, measured differentially** (hold a direction from well
before actionability, compare `x` per frame against the identical run with no
input — docs/frames.md's "no ABSOLUTE observation of motion means anything";
3 reps, identical):

| fighter | first frame `x` diverges from the no-input control |
|---|---|
| Reptile (the attacker, after his own slide) | **f53** |
| Mileena (the victim, after the knockdown) | **f68** |

**The knockdown is worth +15 frames at a 160 px gap.** Knockdown itself is
confirmed on the victim's OWN `y` (87 → 42 → 87 over f12–f41), never against a
scalar ground line (docs/frames.md §10).

**Controls (3 reps each, identical).**

| control | result |
|---|---|
| 5-C1: slide alone | P1 −13 at f11, knockdown, settles at a **160 px** gap |
| 5-C2: slide **BLOCKED** | P1 −3 chip at f11, **no** `y` excursion, settles at **62 px** — the blocked slide leaves him point blank, the hit one throws the fight back to full screen |

### REFUSED: "the attacker is standing over Mileena on wakeup with jab pressure"

Named reason: **launch distance vs connect range**. The slide launches the
victim to a 160 px gap (identical from 4 rungs tested). Reptile's longest
connect range is 110 px. He can walk again at f53, she at f68 — 15 free
frames, and at ~2.5 px/frame those 15 frames buy ~38 px of the 83 px he needs.
In the executed sequence his first threatening button contacts at **f97, 29
frames AFTER she is actionable**. There is no meaty here and no okizeme in the
usual sense. What the knockdown genuinely buys is the FREE APPROACH, and that
is what the beats claim and what the numbers support: he arrives at 77 px and
opens with his own +13-on-block jab loop — sequence 1's pattern, handed to the
other side.

---

## Cross-defender findings (Reptile rows re-verified against a Mileena defender)

Reptile's rows in `arcade.frames.json` were measured on a Reptile-vs-Reptile
mirror. Blockstun is a DEFENDER-side quantity, so any Reptile-as-attacker
advantage used here had to be re-measured against Mileena.

| row | mirror table | measured vs Mileena | note |
|---|---|---|---|
| `reptile/HK/far` on_block | **−16** | **−17** (Mileena's earliest press f28, Reptile's f45, at gap 71 → 95 px) | 1 frame more negative. Classification unchanged (punishable either way), but the mirror number is not exactly reproduced against this defender. |
| `reptile/HK/far` on_hit | +7 | **NOT MEASURED — refused** | the hit leaves a 150 px gap and Mileena's longest connect range is 114 px, so the press-sweep rig has no button to sweep. Named reason: RANGE. |
| `reptile/HP/close` damage / FAF | 24, FAF 8 (medium confidence, sample_n 1) | **24 at press+8** at gap 61 px | reproduced against Mileena. |
| `reptile/HP/far` connect_range | 72 | **connects at 77 px** vs Mileena (sequence 5, f97 and f116, 3 chip each) | connect range is a hurtbox pair property; the mirror's 72 px bound does not transfer to this defender. |
| `reptile/slide` on_block | −12 | not re-measured | the demo uses the slide ON HIT only. |

---

## What this report does not claim

- No FAF, active-window, recovery or hitstop number is re-derived here. The
  beats reproduce the SHAPE `arcade.frames.json` already measured; the
  frame-exact numbers live in that file.
- The press-sweep rig's "earliest actionable" is "earliest frame a 2-frame
  button assertion produces contact". It is a second RIG, not a second
  observable, which is why its agreement on four rows is worth something —
  and why its 6-frame disagreement on the roll is reported as a disagreement
  rather than resolved in either direction.
- Beat expectations are checked by `choreo.verify` on contact presence,
  struct-health deltas, rig-known guard state and the punish damage-register
  read. All 20 beats across the 5 sequences PASS, with zero unchecked
  (`None`) expectations.
