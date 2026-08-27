# RECORDER V3 — the port-pipeline contract

Status: **CONTRACT** (W0). The four W1 agents implement against this document.
It amends `shadow/SPEC.md` §5 (jsonl-v2) and `docs/game-profiles.md`. Where this
document and code disagree, this document wins; where this document is silent,
current behavior stands. RFC-2119 keywords are normative.

Problem being solved: `src/record.rs` is asurabld-shaped (a hardcoded `Fighter`
struct, a recorder-private gate composite, literal offsets) and `library/<fam>/`
can hold only one port profile. Game #3 is MK2 with two ports — arcade
(`library/mk2/mk2.profile.json`, TMS34010, partial map) and Genesis (68k, WRAM
at 0xFF0000, profile landing as `library/mk2/genesis.profile.json`). The goal:
full record→fit→playback on Genesis first, with recordings/models/arenas shared
per-FAMILY across ports.

---

## 1. jsonl v3 — the line schema

### 1.1 Version marker and detection

- Every v3 data row MUST begin with `"v": 3` (first key of the object).
- Readers MUST detect per file, from the first parseable row:
  - has `"v": 3` → v3;
  - no `"v"` but has `"block1"` → v2 (asurabld-shaped; still readable, §3);
  - neither → reject with the existing v1 error.
- The recorder MUST only ever write v3. There is no flag to write v2.

### 1.2 Row shape

One JSON object per emulated frame, 60 Hz, streaming-appended, raw values only
(normalization stays a train-time concern — SPEC §5 unchanged).

Concrete v3 row, asurabld (every value raw, exactly as read):

```jsonc
{"v":3,"frame":918273,"round_id":3,"controllable":true,"p1_block":1,
 "block1":{"timer":3541,"anim":12,"action":9,"x":122,"y":216,"facing":1,
           "weapon":0,"health":239,"health2":239,"meter":64,"meter_max":81,
           "char_id":1,"wins":0,"opp_right_hold":0,"opp_left_hold":0},
 "block2":{ /* same keys, base block2 */ },
 "globals":{"round_over":0,"abort":0,"match_end":0,"round_timer":133,
            "char_select":0,"combo_on_b2":0,"combo_on_b1":0,"demo_flag":1,
            "credits":8},
 "p1_input":128,"p2_input":0}
```

Illustrative v3 row for a sparser port (MK2 Genesis — field names are whatever
its profile ends up mapping; this shows the degradation, not the final map):

```jsonc
{"v":3,"frame":1204,"round_id":1,"controllable":true,"p1_block":1,
 "block1":{"char_id":1,"health":161,"x":213},
 "block2":{"char_id":9,"health":140,"x":301},
 "globals":{"screen_state":0,"round_over":0},
 "p1_input":16,"p2_input":0}
```

Normative rules:

1. **`block1`/`block2` carry EXACTLY the profile's `memory.fighter_fields`,
   by name, in profile order.** No fixed `Fighter` struct. A field the profile
   does not map is ABSENT from the row — never emitted as 0. `size: 1` fields
   read as u8; `size: 2` as u16 in the profile's `memory.endianness`. Values
   are raw (no id translation — §6; raw fields stay raw on disk).
2. **`globals` carries the raw value of every profile global the recorder
   samples**, keyed by the profile's global NAME (not v2's ad-hoc names
   `timer_bcd`/`char_sel`): the union of (a) every global referenced by a
   `gate` condition and (b) every entry of the new `record_globals` profile
   key (§2.1), in that order (gate order first, then `record_globals` order;
   duplicates appear once, at first position). Read size: gate `word_zero`
   globals are u16 (guest order); gate `byte_zero`/`bcd_valid_nonzero` are u8;
   `record_globals` entries carry their own `size`. An empty union MUST still
   emit `"globals":{}`.
3. **`controllable` is `training::eval_gate(ds, profile)` — the one gate.**
   The recorder MUST NOT keep a private composite. Same evaluator as training
   enforcement and Lua `game.controllable()`. (For asurabld this is
   value-identical to the v2 composite: the profile's six conditions ARE the
   v2 composite — that identity is what makes §3's G2/G3 achievable.)
4. **`round_id` / `p1_block`**: unchanged edge semantics — `round_id`
   increments on each gate false→true edge; `p1_block ∈ {1,2,null}` is
   resolved at that edge and sticky for the round. Anchor policy:
   - if the profile maps fighter field `x`: smaller `x` = left = P1 (v2 rule);
   - else: `p1_block` = 1 (fixed-slot assumption), and the meta sidecar MUST
     record `"anchor": "fixed_slots"` so the honesty is on disk.
5. **`p1_input`/`p2_input`**: unchanged — authoritative 12-bit RETRO masks
   captured at the frontend input layer.
6. **Determinism**: key order within a row is fixed as shown (`v, frame,
   round_id, controllable, p1_block, block1, block2, globals, p1_input,
   p2_input`); block keys in profile `fighter_fields` order; globals per rule
   2. Two recorders on identical state MUST emit identical bytes (this is
   what lets G3 assert on serialized lines).

### 1.3 `.meta.json` provenance sidecar (v3)

Written once at create, next to the recording. It MUST snapshot everything the
Python side needs to interpret the file WITHOUT loading the same profile the
recorder had — this is what makes cross-port fits honest (§4.3).

```jsonc
{
  "format": "jsonl-v3",
  "family": "mk2",
  "port": "genesis",
  "profile_file": "genesis.profile.json",
  "profile_sha256": "…",           // sha256(port-profile bytes ‖ family.json bytes)
  "game": "mk2",                   // free-form provenance (ROM), as today
  "core": "genesis_plus_gx",       // free-form provenance, as today
  "style": null,                   // recorder style tag, as today
  "fps": 60,
  "anchor": "smaller_x",           // or "fixed_slots" (§1.2 rule 4)
  "blocks": { "block1": "0xFF8000", "block2": "0xFF8200", "stride": "0x200" },
  "fighter_fields": [ {"name":"char_id","off":"0x0","size":1}, … ],  // verbatim from profile
  "globals_recorded": [ {"name":"screen_state","size":2}, … ],       // union of §1.2 rule 2
  "gate": [ {"kind":"word_zero","global":"screen_state"}, … ],       // verbatim from profile
  "calibration": { "HEALTH_MAX": 161, "SCREEN_W": 320, … },          // verbatim from profile
  "created": "2026-08-27T…Z"
}
```

The v2 fields `blocks`/`gate`(string)/`anchor`(string)/`note` are superseded by
the above; `game`/`core`/`style`/`fps` keep their v2 meaning.

### 1.4 `.rounds.jsonl` sidecar (v3)

Same event semantics as v2 (one line per gate falling edge + partial on
finish). Field names are kept so existing coverage tooling keeps parsing;
two additions and one semantic change:

```jsonc
{"round_id":1,"block1_char":1,"block2_char":7,"p1_block":1,"frames":2412,
 "p1_input_mass":40312,"demo":false,"style":"rushdown","family":"mk2",
 "port":"genesis","v":3}
```

- `port` (NEW): the recording port.
- `v: 3` (NEW): sidecar schema marker.
- `block1_char`/`block2_char` are **CANONICAL ids** (§6), translated through
  `GameProfile::canon_char_id` at write time. For asurabld (no `id_map`) this
  is the identity, so v2 and v3 sidecars agree byte-for-value. Coverage,
  matchup slugs, and model-set keys therefore work across ports unchanged.
- If the profile maps no `char_id` fighter field, `block1_char`/`block2_char`
  MUST be `null` (never 0).

---

## 2. Profile schema additions

Three OPTIONAL keys. Absent keys mean exactly what today's profiles mean —
every shipped profile remains valid unedited (though asurabld's SHOULD be
updated, below).

### 2.1 `memory.record_globals` — extra per-frame sampled globals

```jsonc
"memory": {
  …,
  "record_globals": [
    { "name": "combo_on_b2", "size": 1 },
    { "name": "combo_on_b1", "size": 1 },
    { "name": "demo_flag",   "size": 2 },
    { "name": "credits",     "size": 1 }
  ]
}
```

- Each `name` MUST exist in `memory.globals` (load-time validation, both
  loaders, same error style as gate validation:
  `record_globals names unknown global '<name>'`).
- `size` MUST be 1 or 2. Read per `memory.endianness`.
- Purpose: analysis/train-time signals that are not gate conditions (v2's
  `demo_flag`, `credits`, combo counters). Default: empty list.

### 2.2 `hitstun_sources` — where hitstun evidence lives (top level)

```jsonc
"hitstun_sources": { "block1": "combo_on_b1", "block2": "combo_on_b2" }
```

- Maps block name → the global whose RECENT CHANGE means that block's fighter
  is in hitstun (dataset.py's `_recent_change_mask` contract, unchanged).
- Both names MUST appear in the §1.2-rule-2 recorded-globals union (load-time
  validation: `hitstun_sources names unrecorded global '<name>'`).
- Absent → the family has no hitstun feature; `me_hitstun`/`opp_hitstun` are
  dropped from the vector (§4.2) and `_bucket()` degrades (no
  offense/defense buckets — they require the hitstun bits).

### 2.3 `id_map` — raw RAM char id → canonical roster id (top level)

See §6. Schema:

```jsonc
"id_map": { "5": 3, "6": 4, "12": 11 }
```

Keys are RAW ids as decimal strings (JSON objects can't have int keys — same
convention as `stage_select.value_to_home_char`); values are canonical
`family.json` roster ids. Absent map or absent key = identity. Values MUST
exist in the family roster (load-time validation:
`id_map maps to unknown roster id <N>`).

### 2.4 Required asurabld profile edits (A1, one commit with the recorder)

`library/asurabld/asurabld.profile.json` MUST gain, so v3 preserves every
signal v2 recorded:

- `record_globals` exactly as the §2.1 example (order as shown — it fixes the
  serialized `globals` order);
- `hitstun_sources` exactly as the §2.2 example;
- two `fighter_fields` appended (v2's analysis-only accumulators):
  `{"name":"opp_right_hold","off":"0x28","size":2}`,
  `{"name":"opp_left_hold","off":"0x2A","size":2}`.

No `id_map` (identity). `library/mk2/mk2.profile.json` needs no edit for v3 to
function (it records honestly sparse rows); the Genesis agent authors
`genesis.profile.json` to this schema.

---

## 3. Back-compat gates (non-negotiable)

### G1 — the golden: byte-identical refit of goat-v2 from v2 recordings

The golden is the tracked model **`shadow/models/asurabld/goat-v2/`**
(`cases.npz` + `meta.json`; kept tracked because `shadow_runner` tests read
it). After A2's changes land, this refit on the machine holding the original
v2 recordings (they are gitignored — G1 is a dev-machine acceptance gate, not
CI):

```sh
# meta.json's source_files predate the per-family recordings layout — insert
# the /asurabld/ segment (files verified present there 2026-08-27):
cd shadow/train && python -m shadow_train fit \
  $(python -c "import json;print(' '.join(p.replace('recordings/','recordings/asurabld/') for p in json.load(open('../models/asurabld/goat-v2/meta.json'))['source_files']))") \
  --out /tmp/goat-v2-refit --k 15
```

(The refit's `meta.json.source_files` will carry the new paths — exclude
`source_files` alongside `created` in the meta comparison below.)

MUST reproduce the golden:

- `cases.npz`: extracted-array identity — same key set; per key, identical
  dtype, shape, and `tobytes()`. (Container bytes MAY differ by zip metadata;
  array bytes may not. `sha256` of the whole file matching is sufficient but
  not required.)
- `meta.json`: identical after deleting the `created` and `source_files` keys
  from both (paths moved; every other key — counts, calibration, classes,
  feature_names — MUST match exactly).

Determinism is already in place (fixed `seed=11` neutral subsample, fixed k);
A2 MUST NOT introduce any new nondeterminism into the v2 path.

### G2 — v3-on-asurabld features ≡ v2 features (CI)

A committed fixture proves the v3 reorganization changes nothing for asurabld:

- Fixture: **`shadow/train/tests/fixtures/v2-asurabld-sample.jsonl`** — A2
  creates it by truncating a real v2 recording to ≥2 complete non-demo rounds,
  ≤1 MB, committed (the fixtures dir is exempt from the recordings gitignore).
- Transcoder: `shadow_train` test helper `transcode_v2_to_v3(row) -> row`
  implementing §1.2 mechanically for the asurabld profile: add `"v":3`; blocks
  pass through (v2's Fighter key SET equals the §2.4 fighter_fields names;
  key order is irrelevant to the Python reader, so no reorder is needed);
  `gate` object → `globals` object with the key renames
  `timer_bcd → round_timer`, `char_sel → char_select`, reordered per §1.2
  rule 2. Nothing else changes.
- Test (CI): `build([v2_fixture])` vs `build([transcoded_v3_fixture])` MUST
  produce identical `y_move`, `y_attack`, `buckets`, `rounds`, and
  `X` matrices with `max |ΔX| ≤ 1e-12` (they should be bit-equal; 1e-12 is
  the allowed slack, not a license).

### G3 — Rust writer conformance (CI)

Extend `src/record.rs` tests: on a synthetic `DebugState` with known bytes
(the existing bus-window test pattern), the v3 recorder MUST emit:

- rows whose serialized field order and names match §1.2 rule 6 exactly for
  the asurabld profile (assert on the JSON text of one row);
- `controllable` equal to `training::eval_gate` on the same state for at least
  one open-gate and one closed-gate frame (locks the recorder to the one
  evaluator forever);
- for a stub/partial profile (load `library/mk2`), rows containing ONLY the
  mapped fields (assert `"y"` absent, `"health"` present) — the no-zero-lies
  rule under test.

Python-reads-v2 regression: the existing dataset tests plus G1/G2 cover it;
A2 MUST keep the v2 reader path exercised by at least one CI test (the G2
fixture in its v2 form serves).

---

## 4. Python contract (`shadow_train`)

### 4.1 Reader

- `dataset._rounds` gains per-file version detection (§1.1) and a row-accessor
  layer; everything downstream consumes accessors, not raw dict shapes:
  - `fields(row, "block1") -> dict` (v2: the fixed struct; v3: the named map);
  - `global_value(row, name)` (v2: `row["gate"]` with the reverse renames of
    §3-G2; v3: `row["globals"]`).
- v1 rejection unchanged. Mixed v2/v3 across FILES in one fit is allowed
  (that is G1+new-recordings life); mixed versions within one file are not
  detected and not supported.

### 4.2 Feature vector from named fields — availability, not zero-fill

`SCALAR_FEATURES` stops being a constant and becomes **profile-derived**: the
canonical ordered list below, filtered to features whose requirements the
recording satisfies. Order NEVER changes; entries only drop out. Shapes never
mix because models are per-family and `meta.json.feature_names` stays
authoritative for any loaded model (runner already sizes from it).

| feature(s) | requires | on missing |
|---|---|---|
| `dist_x` | field `x` (+ `X_SCALE`) | REQUIRED — fit aborts: `profile maps no 'x' fighter field — cannot build features` |
| `dy`, `me_airborne`, `me_height`, `opp_airborne`, `opp_height` | field `y` + `GROUND_Y`, `Y_SCALE` | dropped |
| `me_fwd_hold`, `me_back_hold` | input masks | always present |
| `me_anim`, `opp_anim` | field `anim` + `ANIM_SCALE` | dropped |
| `me_timer`, `opp_timer` | field `timer` + `TIMER_SCALE` | dropped |
| `facing_sign` | — (see s rule) | always present |
| `me_health`, `opp_health`, `health_lead` | field `health` + `HEALTH_MAX` | dropped |
| `me_meter`, `opp_meter` | fields `meter` AND `meter_max` | dropped |
| `me_hitstun`, `opp_hitstun` | `hitstun_sources` (§2.2) | dropped; `_bucket()` loses offense/defense |
| `me_corner` | field `x` + `CORNER_PX`, `SCREEN_W` | present (x is required) |

- **Facing sign `s`**: field `facing` if mapped, else the position fallback
  `s = sign(opp.x − me.x)` (SPEC §2's old rule, now a first-class degradation;
  with fallback `s`, `dist_x = |Δx|` and fwd/back holds are position-relative).
- **Required fields per fit**: `x`, `char_id`, `health` (matchup machinery and
  the demo/gate semantics need them). A recording lacking any of the three
  aborts the fit with the field named.
- Categorical passthroughs `me_action`/`opp_action` become `None`-valued when
  `action` is unmapped; `me_char`/`opp_char` come from `char_id` and are
  **canonical** ids (§6) — the ONE Python translation point is in
  `_decisions_for_round` where `Decision.me_char/opp_char` are built.

### 4.3 Calibration comes from the RECORDING, not the process profile

v3's meta sidecar snapshots the port's `calibration` (§1.3). The dataset MUST
normalize each recording with **its own sidecar's calibration**, falling back
to the loaded profile's for v2 files (identical for asurabld, so G1 holds) and
for v3 files whose sidecar is missing (warn once per file). Cadence keys
(`P`, `K`, `STALE`, `SEGMENT_DECISIONS`, `HITSTUN_RECENT_FRAMES`) are
FIT-GLOBAL and always come from the loaded profile — only scaling keys
(`GROUND_Y`, `X_SCALE`, `Y_SCALE`, `TIMER_SCALE`, `ANIM_SCALE`, `CORNER_PX`,
`HEALTH_MAX`, `SCREEN_W`) are per-recording. This is what makes a mixed
arcade+Genesis fit express both ports in one normalized space.

Mixed-port fit rule: all recordings in one fit MUST yield the same
feature-name list; otherwise abort listing the difference per file
(`feature sets differ: session-A lacks [dy, me_height] vs session-B`).
Model `meta.json`: `port` as today when uniform; else `"port": "mixed"` plus
`"ports": ["arcade","genesis"]`. Deploy's port-mismatch warning treats
`mixed` as matching every port of the family.

### 4.4 What explicitly does NOT change

Decision cadence and labels (§SPEC 3/4), demo filtering (zero `p1_input`
mass), segmentation, neutral capping, kNN fit/save, the eval stack, the
coverage/index commands' interfaces. Module-level calibration constants stay
exported (loaded-profile values) for the fit-global keys.

---

## 5. Multi-port profile layout and selection

### 5.1 Layout

```
library/mk2/family.json            # one family vocabulary (canonical ids, §6)
library/mk2/mk2.profile.json       # arcade — LEGACY NAME KEPT, stays the default
library/mk2/genesis.profile.json   # second port
library/mk2/mk2.md                 # evidence doc (shared; per-port sections)
```

Data roots stay per-FAMILY and SHARED across ports: `shadow/models/mk2/`,
`shadow/recordings/mk2/`, `shadow/arenas/mk2/`. That sharing is the point
(the cross-port experiment) and requires §6.

### 5.2 Selection: the `--game` path grows one optional segment

Chosen: **path-segment selection** (`--game library/mk2/genesis`), not a
`--port` flag. Justification: every surface that names a game already passes
exactly one path — `--game`, `RUSTRETRO_GAME_DIR`, `profile.load(game_dir)`,
`shadow/play.py`, `loop.sh`'s FAMILY env. One smarter path keeps all of them
single-argument; a second flag would have to be threaded through five
surfaces and two languages for zero added expressiveness.

Loader behavior (`GameProfile::load` in `src/profile.rs`, mirrored EXACTLY by
`shadow_train.profile.load`) — this replaces the current "else the single
`*.profile.json`" fallback:

1. `dir` is a directory → family dir = `dir`; profile file =
   `dir/<dirname>.profile.json` if it exists (legacy default — asurabld and
   mk2-arcade keep working unedited);
   else if exactly ONE `*.profile.json` exists in `dir`, use it;
   else if none: error `"<dir>: no *.profile.json found"` (unchanged);
   else (≥2, no dirname default): error
   `"<dir>: multiple port profiles and no <dirname>.profile.json default — select one: --game <dir>/<a|b|…>"`
   listing the candidate stems. (The old silent first-match is WRONG with two
   ports and MUST be removed.)
2. `dir` is NOT a directory but `dir.parent` is → family dir = parent,
   selector = basename:
   a. `parent/<basename>.profile.json` exists → use it;
   b. else scan `parent/*.profile.json` for files whose parsed `"port"` field
      equals the basename; exactly one match → use it (so
      `--game library/mk2/arcade` resolves `mk2.profile.json`); zero → error
      `"<parent>: no port '<basename>' (no <basename>.profile.json and no profile with \"port\": \"<basename>\"); available: <stems/ports>"`;
      two+ → error `"<parent>: port '<basename>' is ambiguous: <files>"`.
3. Neither → error `"--game <path>: no such game directory"`.

Everything downstream is unchanged: `family.json` still loads from the family
dir, the family≠profile cross-check stays, data roots key off
`family.json.family`. Scope cap: this is `GameProfile::load` (+ its Python
mirror) and `main.rs` `--game` help text ONLY.

---

## 6. Cross-port identity: canonical char ids

**`family.json` roster ids are the CANONICAL ids of the family.** For mk2 they
are the arcade ids already shipped (0=kunglao, 1=liukang, 3=baraka, …). New
characters discovered on any port are appended to the family roster with fresh
canonical ids; a port whose RAM uses different numbers carries `id_map`
(§2.3) translating raw → canonical.

Rules:

- **Raw stays raw in traces**: v3 row `char_id` fields are untranslated RAM
  bytes (§1.2 rule 1 — recordings are evidence).
- **Everything derived is canonical**: `.rounds.jsonl` chars, matchup slugs
  and model-set keys, coverage matrices, `Decision.me_char/opp_char`,
  stage-select opponent forcing, panel display names.
- **Exactly one translation point per language, by name**:
  - **Rust**: `profile::GameProfile::canon_char_id(&self, raw: u8) -> u8`
    (identity when `id_map` absent/misses). Owned by A4. Every Rust consumer
    of a RAM-read char id calls it immediately after the read — call sites:
    `record.rs` (rounds sidecar), `shadow_runner` (model-set round-start
    switching), the matchup panel. No other Rust code may touch `id_map`.
  - **Python**: `shadow_train.profile.GameProfile.canon_char_id(raw)` —
    called ONLY in `dataset._decisions_for_round` when constructing
    `Decision`. Filters (`--char/--opp`), coverage, and slugs therefore
    operate on canonical ids for free.
  - **Lua**: none. Lua bindings that expose char ids (`game.*`) return values
    already translated inside their Rust implementations — Lua never sees a
    raw id and never re-implements the map.
- `stage_select.value_to_home_char` values and `enforcement` are per-port and
  already live in the port profile; they are expressed in CANONICAL ids (the
  arcade profile's existing table is canonical by definition).

Consequence, stated for the record: an arcade-fitted `liukang-vs-reptile`
model is retrievable by the Genesis runner the moment Genesis rounds report
the same canonical pair — matchup keys, coverage cells, and arena slugs are
port-blind. Whether the FEATURES transfer across ports is the experiment
(§4.3's mixed-fit rule is the honest guardrail), but identity never lies.

---

## 7. Ownership map (W1)

| agent | files owned (writes) | must NOT touch |
|---|---|---|
| **A1** (fable) — recorder v3 | `src/record.rs` (rewrite: profile-driven Fighter/globals, eval_gate, canonical sidecar, v3 meta), `src/frontend.rs` (`set_recorder` plumbing only), `library/asurabld/asurabld.profile.json` (§2.4 additions), G3 tests | `src/training.rs` (calls `eval_gate` read-only), `src/profile.rs` |
| **A2** (sonnet) — Python v3 | `shadow/train/shadow_train/dataset.py`, `profile.py` (id_map + §5.2 path mirror + §2 validation), `__main__.py` (mixed-port meta, sidecar `port`), `tests/` + `tests/fixtures/v2-asurabld-sample.jsonl`, G1/G2 | Rust; `library/*` profiles |
| **A4** (haiku) — loader | `src/profile.rs` (§5.2 load, §2 schema structs + validation, `canon_char_id`), `src/main.rs` (`--game` help/error text) | `src/record.rs`, Python |
| **A3** (parallel RE) — Genesis port | `library/mk2/genesis.profile.json` (new), `library/mk2/family.json` (roster append + id_map source data), `library/mk2/mk2.md` evidence | all code |

Boundary resolutions (files two agents would both need):

- `src/profile.rs`: A1 and A2 both DEPEND on the new schema keys and
  `canon_char_id`; A4 OWNS the file exclusively. §2's schemas and the
  `canon_char_id` signature are fixed HERE so A1/A2 code against this
  document without waiting; merge order A4 → A1.
- `shadow_train/profile.py`: both §5.2 mirroring and §6 could argue for a
  Rust-side sibling owner; resolved by giving ALL Python (including the
  loader mirror) to A2 — the mirror's spec is §5.2 verbatim, no coordination
  needed.
- `src/training.rs` is read-only for everyone (its `eval_gate` is already
  `pub(crate)`); if A1 needs visibility changes, that one-line edit is A1's.
- `library/asurabld/asurabld.profile.json` is A1's (its content is coupled to
  the recorder's output order); A3 owns everything under `library/mk2/`.
