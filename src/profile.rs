//! Game profiles: per-game knowledge as data instead of compiled constants.
//!
//! Two-tier schema (see docs/game-profiles.md for the design contract):
//!   library/<game>/family.json           — port-independent vocabulary:
//!     roster, move/attack class lists, block style. Shared by every port of
//!     the game and by trained models (meta.json carries family+port).
//!   library/<game>/<game>.profile.json   — port binding: core identity +
//!     capability prerequisites, memory map (blocks, fighter field offsets,
//!     named globals), the controllable-gate condition list, enforcement
//!     values, stage/opponent selector, feature calibration, attack chords.
//!
//! The profile is loaded ONCE at startup (`init`, from `--game <dir>`,
//! default `library/asurabld`) into a process-wide `OnceLock`; call sites
//! use `profile::current()`. This deliberately mirrors how the previous
//! compiled constants behaved (one game per process) while making the game
//! swappable at launch. The Python side reads the SAME JSON files
//! (`shadow_train.profile`), which is what keeps the Rust runner and the
//! Python trainer describing one reality — the successor to the old
//! "hand-kept in four places" rule: now there is one place, and it is data.
//!
//! Addresses serialize as hex strings ("0x403798") for legibility, matching
//! the busmap sidecar convention.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

// ── family.json ─────────────────────────────────────────────────────────────

// Some schema fields are contract surface read by the Python/Lua sides or
// by serde validation only — not (yet) by Rust code. That is by design.
#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
pub struct Family {
    pub family: String,
    #[serde(default)]
    pub title: String,
    pub roster: Vec<RosterEntry>,
    pub move_classes: Vec<String>,
    pub attack_classes: Vec<String>,
    #[serde(default)]
    pub block: BlockStyle,
    /// Family-level move vocabulary (shadow/MACRO_ACTIONS.md §1), keyed by
    /// canonical roster NAME. A character absent here simply has no specials;
    /// an absent map keeps the whole macro layer off (asurabld this phase).
    #[serde(default)]
    pub moves: BTreeMap<String, Vec<MoveDef>>,
}

/// One named family-level move intent. Tags are open strings; "special"
/// marks label-space membership on the Python side.
#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
pub struct MoveDef {
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
pub struct RosterEntry {
    pub id: u8,
    pub name: String,
    /// Char-select cursor position (Rights from default); None = not on the
    /// select screen (bosses / hidden characters).
    #[serde(default)]
    pub select_slot: Option<u8>,
    #[serde(default)]
    pub boss: bool,
}

/// How blocking works in this game family. `back_hold` (SF/Asura style) vs
/// a dedicated held button (MK style, named by its attack class).
#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone, Default)]
pub struct BlockStyle {
    #[serde(default = "d_block_style")]
    pub style: String,
    /// For style == "button": which attack-class name is the block button.
    #[serde(default)]
    pub class: Option<String>,
}
fn d_block_style() -> String {
    "back_hold".into()
}

// ── <game>.profile.json ─────────────────────────────────────────────────────

#[derive(Deserialize, Debug, Clone)]
pub struct PortProfile {
    pub family: String,
    pub port: String,
    pub core: CoreInfo,
    #[serde(default)]
    pub requires: Requires,
    pub memory: MemoryMap,
    pub gate: Vec<GateCond>,
    pub enforcement: Enforcement,
    #[serde(default)]
    pub stage_select: Option<StageSelect>,
    /// Feature-scaling constants; keys match `shadow_train.dataset` names.
    pub calibration: BTreeMap<String, f64>,
    /// Attack-class name -> RETRO button names held simultaneously.
    pub attack_chords: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub positions: BTreeMap<String, u32>,
    /// Where hitstun evidence lives: block name ("block1"/"block2") -> the global
    /// whose recent change indicates hitstun for that block's fighter.
    #[serde(default)]
    pub hitstun_sources: Option<BTreeMap<String, String>>,
    /// Raw RAM char id -> canonical roster id. Absent map or absent key = identity.
    /// Keys are decimal strings (JSON object constraint). Values must exist in family roster.
    #[serde(default)]
    pub id_map: Option<BTreeMap<String, u8>>,
    /// RAM values the app holds for the whole session (re-asserted ~1 Hz),
    /// independent of training mode and the gate — for settings the game
    /// keeps in volatile RAM that must not silently reset on a cold boot
    /// (MK2 Genesis: the per-port 6-button pad flags). `freeze` can't do
    /// this on direct-pointer regions; periodic writes are the mechanism.
    #[serde(default)]
    pub pins: Vec<Pin>,
    /// Port-level special-move encodings (shadow/MACRO_ACTIONS.md §2):
    /// character name -> move name -> ordered macro steps. OMISSION IS
    /// MEANINGFUL — a port that lacks a move omits its entry and every
    /// consumer offers only what the port encodes.
    #[serde(default)]
    pub special_inputs: BTreeMap<String, BTreeMap<String, Vec<StepSpec>>>,
    /// Global "somebody was just struck" signal (hit OR block), used by the
    /// block-punish dummy when `hitstun_sources` is absent (MK2 arcade's
    /// hit_counter). Neither mapped → BlockPunish degrades per-feature.
    #[serde(default)]
    pub contact_signal: Option<ContactSignal>,
    /// Port-level guard tuning for back-to-block families (MACRO_ACTIONS §9).
    /// Absent → the reactive guard falls back to its built-in default range.
    #[serde(default)]
    pub block: Option<PortBlock>,
}

/// Port-level guard data (MACRO_ACTIONS §9.2). `guard_range` is the only
/// value code reads; the `*_verdict`/`*_evidence` strings are the live-RE
/// provenance the schema carries deliberately (the .md holds the long form)
/// so a future port can't silently inherit asurabld's conclusions.
#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
pub struct PortBlock {
    /// Max |opp.x − me.x| (units of the mapped `x`) at which the guard window
    /// may open. Beyond it the dummy ignores the attack — that is what keeps
    /// it from reacting to far whiffs.
    #[serde(default)]
    pub guard_range: Option<u32>,
    #[serde(default)]
    pub guard_range_evidence: Option<String>,
    /// Whether crouch-guard (down-back) works on this port. asurabld: it does
    /// NOT (0/4, reproduced) — the guard hold must never add Down.
    #[serde(default)]
    pub overhead_verdict: Option<String>,
    /// Whether holding away silently charges specials (§9.5).
    #[serde(default)]
    pub charge_hazard_verdict: Option<String>,
}

/// The contact-signal declaration: something whose CHANGE means "this
/// fighter was struck (hit OR blocked)". Exactly one source:
/// - `field`: a per-fighter field name, resolved per block — PREFERRED,
///   because it is per-victim by construction. MK2 arcade ships struct
///   `health` (block+0x0E): it steps by the whole damage in ONE frame, on
///   hit AND on block — blocked normals always chip on this port (3/6/8,
///   mk2.md "Hitstun / blockstun observables") — unlike the drawn HUD pair,
///   which animates 1 unit/frame and smears one hit into ~11 edges. (The
///   earlier `action_counter` contact claim was RETRACTED — mk2.md.)
/// - `global`: one address shared by both fighters (weaker: usually
///   victim-asymmetric, as MK2's hit_counter turned out to be).
///
/// `direction` (optional) restricts which changes COUNT as contact:
/// - `"decrease"`: only a drop in the value is contact. This is what makes a
///   health-valued signal immune to the two INCREASE hazards — the round-
///   intro ramp (+2/frame under the banner-gate leak) and the training
///   refill's write back to max — by one sign check.
/// - absent: any change counts (back-compat; asurabld's combo counters
///   INCREASE on hits, so decrease-only must never be a global rule).
#[derive(Deserialize, Debug, Clone)]
pub struct ContactSignal {
    #[serde(default)]
    pub field: Option<String>,
    #[serde(default)]
    pub global: Option<String>,
    #[serde(default)]
    pub direction: Option<String>,
}

/// One macro step (MACRO_ACTIONS §2/§10): held SEMANTIC directions, attack
/// CLASSES pressed together, and the executor's hold length in frames.
///
/// §10.1 extends this with three more fields, all attack-CLASS lists like
/// `press`: `hold` (this step's chord must stay down for `min_frames`
/// continuous frames before the step is satisfied — a release before that
/// FAILS the whole macro, not just this step), `release` (satisfied on the
/// FALLING edge of this chord), and `while_held` (a chord that must ALSO be
/// down for this step to satisfy — the step-scoped stand-in for nesting,
/// e.g. Reptile's Invisibility holding Block across `U U D`). `press`,
/// `hold`, and `release` are mutually exclusive within one step (load-
/// validated) — they name the step's KIND; `while_held` and `dirs` compose
/// with any of them.
#[derive(Deserialize, Debug, Clone)]
pub struct StepSpec {
    #[serde(default)]
    pub dirs: Vec<String>,
    #[serde(default)]
    pub press: Vec<String>,
    #[serde(default)]
    pub hold: Vec<String>,
    #[serde(default)]
    pub release: Vec<String>,
    #[serde(default)]
    pub while_held: Vec<String>,
    #[serde(default = "d_step_frames")]
    pub frames: u8,
    /// Only meaningful (and required) when `hold` is non-empty: the minimum
    /// number of continuous frames the `hold` chord must stay down before
    /// this step is satisfied.
    #[serde(default)]
    pub min_frames: Option<u32>,
}
fn d_step_frames() -> u8 {
    3
}

/// One pinned RAM value: a named global asserted to `value` for the session.
#[derive(Deserialize, Debug, Clone)]
pub struct Pin {
    pub global: String,
    pub value: u8,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
pub struct CoreInfo {
    #[serde(default)]
    pub library_name: String,
    pub provenance_game: String,
    pub provenance_core: String,
    /// Logical button -> the RETRO name this core actually responds to
    /// (e.g. MAME cores call coin "select").
    #[serde(default)]
    pub button_names: BTreeMap<String, String>,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone, Default)]
pub struct Requires {
    #[serde(default)]
    pub memory_regions: bool,
    #[serde(default)]
    pub save_states: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MemoryMap {
    /// Main CPU family: "m68k" (default) or "tms34010". Gates the per-frame
    /// Sek debug capture — FBNeo exports the Sek symbols for EVERY game, so
    /// calling them on a non-68k driver dereferences an uninitialized CPU
    /// context and segfaults (probe-verified on mk2, 2026-08-26).
    #[serde(default = "d_cpu")]
    pub cpu: String,
    /// "big" (68k) or "little". The read helpers consult this instead of
    /// assuming 68k byte order.
    #[serde(default = "d_endianness")]
    pub endianness: String,
    pub blocks: Blocks,
    pub fighter_fields: Vec<FieldSpec>,
    /// Named global addresses ("round_timer" -> 0x40000A). Gate conditions
    /// and code refer to globals by NAME, never by raw address.
    pub globals: BTreeMap<String, HexAddr>,
    /// Extra per-frame sampled globals beyond those in gate conditions.
    /// Each entry names a global and specifies its read size (1 or 2 bytes).
    #[serde(default)]
    pub record_globals: Vec<RecordGlobal>,
}
fn d_endianness() -> String {
    "big".into()
}
fn d_cpu() -> String {
    "m68k".into()
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
pub struct Blocks {
    pub block1: HexAddr,
    pub block2: HexAddr,
    pub stride: HexAddr,
    /// The per-block pointer to a dynamic object-pool entry (docs/frames.md
    /// §5, docs/game-profiles.md "Pointer-resolved fields"): applied
    /// relative to EACH block's own base (block1's pointer word sits at
    /// `block1 + off`, block2's at `block2 + off`) — MK2 arcade's world
    /// X/Y live behind this indirection, not at a block-relative offset.
    /// Absent for games without it.
    #[serde(default)]
    pub object_ptr: Option<ObjectPtr>,
}

/// Per-block pointer declaration (docs/frames.md §5). Reading a field behind
/// it is inherently a LIVE, multi-step operation (dereference, cross-check,
/// then read) — there is no fixed absolute address to cache, which is why
/// this is a decode DESCRIPTION rather than an address like `Blocks`'
/// siblings. See [`GameProfile::object_ptr_field`].
#[derive(Deserialize, Debug, Clone)]
pub struct ObjectPtr {
    /// Signed offset from the fighter block to the raw pointer word (MK2
    /// arcade: `-0xC` — the pointer sits BEFORE the block).
    pub off: SignedHex,
    /// Byte width of the raw pointer word (MK2 arcade: 4, a `u32`).
    pub size: u8,
    /// Closed vocabulary naming the decode from the raw pointer word to an
    /// absolute object-pool address. Currently just `"tms34010_bitaddr"`:
    /// `(word - 0x01000000) >> 3` (docs/frames.md §5). An unrecognized name
    /// decodes to nothing (never a wrong address) — see [`ObjectPtr::decode`].
    pub encoding: String,
    /// `[lo, hi)` validity range for the RAW pointer word. Outside it the
    /// pointer is not live this frame — every field behind it is ABSENT,
    /// never a synthesized 0 (RECORDER_V3 law, docs/frames.md §2.5).
    pub valid_range: [HexAddr; 2],
    /// Staleness cross-check: the byte at `obj + cid_check_off` must equal
    /// the byte at the fighter block's own `char_id` field (offset 0) or the
    /// pointer has gone stale — the pool slot was reused by a different
    /// object — and the read is ABSENT, not a garbage value.
    pub cid_check_off: HexAddr,
}

impl ObjectPtr {
    /// Decode a raw pointer word into an absolute object-pool address.
    /// `None` when it falls outside `valid_range` (not live this frame) or
    /// `encoding` names a decode this build doesn't know — either way,
    /// ABSENT rather than a wrong address.
    pub fn decode(&self, raw: u32) -> Option<u32> {
        if raw < self.valid_range[0].0 || raw >= self.valid_range[1].0 {
            return None;
        }
        match self.encoding.as_str() {
            // TMS34010 bit-address -> byte address (docs/frames.md §5),
            // verified live across cold boots and a mid-session pool move.
            "tms34010_bitaddr" => Some(raw.wrapping_sub(0x0100_0000) >> 3),
            _ => None,
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct FieldSpec {
    pub name: String,
    /// Offset — meaning depends on `via`. Plain form: offset from the
    /// fighter block base. `via: "object_ptr"` form: offset from the
    /// DECODED OBJECT address instead. Exactly one of `off` (with no `via`)
    /// / `globals` / (`via` + `off`) must be present (validated at load).
    #[serde(default)]
    pub off: Option<HexAddr>,
    /// Global-sourced variant: a per-block pair of named globals, for values
    /// that live OUTSIDE the fighter structs. Superseded for MK2 arcade's
    /// world X by the `via: "object_ptr"` form below (the globals it named,
    /// `p1_x`/`p2_x`, were DISPROVEN — see `library/mk2/mk2.profile.json`
    /// `_STATUS`) but kept as a schema form other ports may still use.
    #[serde(default)]
    pub globals: Option<BlockGlobals>,
    /// Pointer-resolved variant (docs/frames.md §5, docs/game-profiles.md):
    /// `"object_ptr"` means `off` is relative to `memory.blocks.object_ptr`'s
    /// DECODED address rather than the fighter block, and every read is a
    /// live multi-step operation (dereference, char-id cross-check, then
    /// read) — see [`GameProfile::object_ptr_field`]. `field_off`/
    /// `field_addr` return `None` for these fields; there is no fixed
    /// address to hand back.
    #[serde(default)]
    pub via: Option<String>,
    /// Sign-extend the read value at its own width (1 or 2 bytes) instead of
    /// zero-extending. Default false. MK2 arcade's `y` (`obj+0x16`) is
    /// signed; smaller = higher, and negative values occur mid-jump.
    #[serde(default)]
    pub signed: bool,
    /// 1 or 2 bytes (guest order per `endianness`) for the field's own
    /// value; unrelated to `object_ptr.size` (the pointer word's width).
    pub size: u8,
}

/// The per-block global names backing a global-sourced fighter field.
#[derive(Deserialize, Debug, Clone)]
pub struct BlockGlobals {
    pub block1: String,
    pub block2: String,
}

/// Entry in `memory.record_globals`: a global name and read size for per-frame sampling.
#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
pub struct RecordGlobal {
    pub name: String,
    /// 1 or 2 bytes (guest order per `endianness`).
    pub size: u8,
}

/// One controllable-gate condition. The vocabulary is fixed and small on
/// purpose — every condition type here is live-verified for at least one
/// game; a game needing logic beyond this vocabulary gets a Lua adapter
/// hook, not a schema extension.
#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GateCond {
    /// u8 at global == 0.
    ByteZero { global: String },
    /// u16 (guest order) at global == 0.
    WordZero { global: String },
    /// `u16 (guest order) at global & mask != mask` — "these bits are not
    /// ALL set". For phase words that are BITFIELDS rather than enums: MK2
    /// arcade's screen_state carries match-type bits (2-human play sets
    /// 0x100 plus varying low bits; 0/257/259/260/276 all observed IN a
    /// fight) and only the COMBINATION 0x06 marks a menu (char select 262,
    /// attract 263). Enumerating in-fight values, then testing a single
    /// bit, were both whack-a-mole that later observations broke — see
    /// mk2.md's three gate revisions.
    WordMaskedNotAll { global: String, mask: HexAddr },
    /// BOTH fighters' `health` field in min..=max.
    HealthInRange { min: u8, max: u8 },
    /// u8 at global is nonzero and both BCD nibbles are decimal.
    BcdValidNonzero { global: String },
}

#[derive(Deserialize, Debug, Clone)]
pub struct Enforcement {
    pub health_max: u8,
    pub refill_below: u8,
    pub timer_hold: TimerHold,
    pub credits_target: u8,
    pub credits_min: u8,
}

/// How training holds the round timer — two declarative forms, because two
/// real layouts exist (docs/game-profiles.md):
///
/// - **Adjacent** (legacy, asurabld/genesis shape): `[seconds byte,
///   subseconds byte]` written to the `round_timer` global and `+1` every
///   tick. Kept verbatim — asurabld's training tests are the proof.
/// - **Located** (MK2-arcade shape, mk2.md "The round timer, closed"): the
///   authoritative store is a countdown TASK RECORD that physically RELOCATES
///   every fight (base 0xD630 on the 2-human rig it was first captured from,
///   but 0xDC42 / 0xE254 in two separate 1P fights, and the code-pointer word
///   varies too), so no fixed address or fixed guard value exists. The
///   record's STRUCTURE is invariant, so it is found each in-fight frame by
///   scanning `[scan[0], scan[1])` (step 2 — records are u16-aligned) for the
///   first base R that satisfies every `record` predicate, then applying each
///   `writes` entry (a u8 at `R + offset`). Each predicate is `offset` +
///   `size` (1|2|4) + exactly one of: `equals` (hex constant), `min`+`max`
///   (inclusive-min, EXCLUSIVE-max range), or `eq_global` (equal to `size`
///   bytes read at that global's address — the drawn-digit cross-check that
///   makes the match unique). All multi-byte reads get the same
///   guest-endianness fix every profile read gets (MK2's region is
///   little-endian). Zero matches → skip silently (menu/transition). The
///   reference instance is MK2 arcade (mk2.md).
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum TimerHold {
    /// Legacy: [seconds byte, subseconds byte] → `round_timer`/+1.
    Adjacent([u8; 2]),
    /// Structural-locator multi-write (relocating task-record, MK2 arcade).
    Located {
        /// `[start, end)` guest-address scan window (end exclusive).
        scan: [HexAddr; 2],
        /// Signature predicates every matching record base must satisfy.
        record: Vec<TimerPredicate>,
        /// u8 writes applied to the first matching base, relative to it.
        writes: Vec<TimerWrite>,
    },
}

/// One structural predicate on a candidate record base `R`
/// ([`TimerHold::Located`]). Reads `size` bytes at `R + offset` (endian-fixed)
/// and tests them against EXACTLY ONE of `equals` / (`min`+`max`) /
/// `eq_global` — validated at load.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TimerPredicate {
    /// Byte offset of the field relative to the candidate base `R`.
    pub offset: HexAddr,
    /// Read width in bytes: 1, 2, or 4 (validated at load).
    pub size: u8,
    /// Equal to this constant (endian-fixed).
    #[serde(default)]
    pub equals: Option<HexAddr>,
    /// Inclusive lower bound of a range (pairs with `max`).
    #[serde(default)]
    pub min: Option<HexAddr>,
    /// EXCLUSIVE upper bound of a range (pairs with `min`).
    #[serde(default)]
    pub max: Option<HexAddr>,
    /// Equal to `size` bytes read at this named global's address (endian-fixed).
    #[serde(default)]
    pub eq_global: Option<String>,
}

/// One u8 written at `base + offset` for the first matching
/// [`TimerHold::Located`] record.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TimerWrite {
    pub offset: HexAddr,
    pub value: u8,
}

#[derive(Deserialize, Debug, Clone)]
pub struct StageSelect {
    pub global: String,
    /// Selector value -> home character id (forced opponent + venue).
    pub value_to_home_char: BTreeMap<String, u8>,
}

// ── hex-string address adapter (busmap convention) ──────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HexAddr(pub u32);

impl<'de> Deserialize<'de> for HexAddr {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            S(String),
            N(u32),
        }
        match Raw::deserialize(d)? {
            Raw::N(n) => Ok(HexAddr(n)),
            Raw::S(s) => {
                let t = s.trim().trim_start_matches("0x").trim_start_matches("0X");
                u32::from_str_radix(t, 16)
                    .map(HexAddr)
                    .map_err(serde::de::Error::custom)
            }
        }
    }
}

/// Signed hex offset (`"-0xC"` or `"0x12"`, or a bare negative/positive
/// integer). Object-pointer declarations need negative offsets — the
/// pointer word sits BEFORE the fighter block (MK2 arcade: `block - 0xC`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignedHex(pub i32);

impl<'de> Deserialize<'de> for SignedHex {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            S(String),
            N(i32),
        }
        match Raw::deserialize(d)? {
            Raw::N(n) => Ok(SignedHex(n)),
            Raw::S(s) => {
                let t = s.trim();
                let (neg, t) = match t.strip_prefix('-') {
                    Some(rest) => (true, rest),
                    None => (false, t),
                };
                let t = t.trim_start_matches("0x").trim_start_matches("0X");
                let v = u32::from_str_radix(t, 16).map_err(serde::de::Error::custom)?;
                Ok(SignedHex(if neg { -(v as i64) as i32 } else { v as i32 }))
            }
        }
    }
}

// ── frame lab data (docs/frames.md) ─────────────────────────────────────────
//
// `library/<family>/<port>.frames.json` is a MEASUREMENTS STORE (§6), not a
// profile constant — exported by the Python harness
// (`shadow_train.framelab`), never authored or edited by Rust. It is
// entirely optional: a game with no export simply has `GameProfile.frames ==
// None`, silently (no warning, §7's "no silent caps" is about a run that
// SKIPPED something, not about a port that never ran the lab at all).
//
// The export currently carries TWO ROWS PER (char, move, variant, gap) cell,
// one per observable (`docs/frames.md` §12) — on MK2 arcade always
// `struct_velocity` and `pointer_x`. The two have agreed on every sweep
// across two independent full runs (52 sweeps in the second alone), so the
// chosen rule is: COLLAPSE agreeing observables into one row. A field where
// the observables DISAGREE is the exceptional case (surfaced rather than
// silently resolved — the collapsed value for that field is left `None` and
// the field's name is recorded in `FrameCell::disagreements`, while each
// observable's own raw value survives unedited in `FrameCell::observations`
// for audit) — EXCEPT that "agree" means something different depending on
// what kind of quantity the field is (`docs/frames.md` §8.4, corrected):
//
// The class is NOT a property of a field's name — it is a property of HOW
// THE ROW WAS MEASURED, specifically whether the value carries a probe
// manifest's manifestation margin (`docs/frames.md` §8.4, corrected a second
// time 2026-09-01):
//
// - **Difference quantities** (`on_hit`, `on_block` — a manifest frame minus
//   another manifest frame, both from the SAME observable) have their
//   observable's own margin cancel out of the subtraction. Two observables
//   must therefore agree EXACTLY. `hitstop` is also a difference quantity —
//   connecting manifest minus whiffing manifest — for the same reason, and
//   for the same reason stays exact even though it is a duration.
// - **Anchor-based quantities** are bracketed by two reads of the SAME
//   anchor signal (§4.1), never a behavioural probe, so neither endpoint
//   carries a manifestation margin. `first_active_frame` (the contact signal
//   relative to a fixed, software-controlled input frame) and `active` (the
//   first and last contact-signal reads across a gap sweep) are both this
//   shape. Held to exact agreement, like a difference quantity, but for a
//   different reason (no margin ever entered the number, rather than two
//   margins entering and cancelling).
// - **One-sided quantities** carry a raw single-sided probe manifest's own
//   manifestation margin `m` directly: `value = A_rel + m`, and — because
//   §3.1 calibrates the OBSERVABLE, not the move — the very same `m` is
//   baked into that observable's `input_latency_frames = l + m` (`l` the
//   shared injection latency). So `value − input_latency_frames = A_rel − l`
//   is invariant across observables measuring the same truth, independent of
//   each one's own `m`. Two sound observables' raw values will therefore
//   differ by exactly the difference in their `input_latency_frames` — NOT
//   by zero. Mileena's roll: `wakeup_window` 77 (`struct_velocity`,
//   latency 1) vs 78 (`pointer_x`, latency 2) is this agreement, not a
//   disagreement (77 − 1 == 78 − 2 == 76). A one-sided field that does NOT
//   satisfy this invariant is a REAL disagreement and is still flagged.
//   `wakeup_window` (an anchor-to-actionable-manifest read for a knockdown)
//   was the first field recognised as this shape. `total` and `recovery`
//   are the SAME shape: under the only measurement protocol this project has
//   (§4, the act-again probe), "recovered" has no anchor signal of its own —
//   it is read the identical way `wakeup_window` is, from a fixed anchor to
//   the actionable-again probe manifest, so it carries exactly one margin
//   too. This was misclassified as anchor-based in the first cut of this
//   rule (`docs/frames.md` §13 item 1): it cost nothing to notice on a
//   contact-anchored move, because nothing has measured `total`/`recovery`
//   there yet, but it blocked every WHIFF-anchored one outright — Reptile's
//   invisibility has no contact to anchor on, so its `total` can ONLY come
//   from the act-again probe, and reads 40 (`struct_velocity`, latency 1) /
//   41 (`pointer_x`, latency 2) — agreement under this rule (40−1 == 41−2),
//   disagreement under the old exact-match rule. Reclassifying is not
//   loosening the check: it is applying the SAME rule already proven correct
//   for `wakeup_window` to a field that is measured exactly the same way.
//
// This is a true statement about the CURRENT protocol, not a permanent fact
// about these field names — if a future observable can read "recovery ended"
// directly off an anchor signal (no probe involved), that measurement would
// be anchor-based instead. The schema has no way to say which shape a given
// row is; today it is inferred from the field name because every row this
// project has produced follows the current protocol. The correct fix is a
// per-row column recording how the duration was bounded (e.g. `anchor_kind:
// "dual_anchor" | "anchor_to_probe"`) so collapsing reads that off the row
// instead of assuming it from the field's name — proposed in
// `docs/frames.md` §12, not implemented here: nothing in this project's
// scope (`shadow_train.framelab`, which would populate it) can be touched
// from `src/profile.rs`, and a column nothing ever writes is not a real fix.
//
// A one-sided field's collapsed value is the raw reading of whichever
// observable in the cell has the SMALLEST `input_latency_frames` — by the
// probe's own construction (`shadow_train.framelab.probe`) that observable's
// margin `m` is zero, so its raw number already equals `A_rel` with nothing
// to correct. `FrameCell::one_sided_reference` records, per such field, which
// observable's frame of reference the collapsed number is in — a bare "77"
// means different things in different rows, and that ambiguity is exactly
// what this map exists to close off.

/// One raw row exactly as `shadow_train.framelab.export` writes it. Field
/// names/nullability mirror the Python schema (`docs/frames.md` §6) —
/// `#[serde(default)]` on every optional field means an export that OMITS a
/// null field (rather than writing `"field": null`) still loads, and an
/// absent value never becomes `0` (§2.5).
#[derive(Deserialize, Debug, Clone)]
struct RawFrameRow {
    family: String,
    port: String,
    char: String,
    #[serde(rename = "move")]
    move_name: String,
    #[serde(default)]
    variant: Option<String>,
    #[serde(default)]
    gap_walk_frames: Option<i64>,
    #[serde(default)]
    gap_px: Option<f64>,
    #[serde(default)]
    first_active_frame: Option<i64>,
    #[serde(default)]
    active: Option<i64>,
    #[serde(default)]
    recovery: Option<i64>,
    #[serde(default)]
    total: Option<i64>,
    #[serde(default)]
    hits: Option<i64>,
    #[serde(default)]
    hitstop: Option<i64>,
    #[serde(default)]
    on_hit: Option<i64>,
    #[serde(default)]
    on_block: Option<i64>,
    #[serde(default)]
    wakeup_window: Option<i64>,
    /// Raw 0/1 (or null = not measured either way). Converted to
    /// `Option<bool>` in [`FrameMeasurement`] — never conflated with the
    /// unmeasured case.
    #[serde(default)]
    knockdown: Option<i64>,
    #[serde(default)]
    juggle: Option<i64>,
    /// Schema reserves this (docs/frames.md §12: NULL in every row measured
    /// so far); kept as raw JSON rather than guessing a concrete type ahead
    /// of the first non-null sample.
    #[serde(default)]
    guard_height: Option<serde_json::Value>,
    #[serde(default)]
    connect_range: Option<i64>,
    #[serde(default)]
    rig_guard_state: Option<String>,
    #[serde(default)]
    damage: Option<i64>,
    observable: String,
    method: String,
    #[serde(default)]
    input_latency_frames: Option<i64>,
    #[serde(default)]
    sample_n: Option<i64>,
    #[serde(default)]
    confidence: Option<String>,
    #[serde(default)]
    measured_at: Option<String>,
    #[serde(default)]
    core_id: Option<String>,
    #[serde(default)]
    rom_id: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    id: Option<i64>,
}

/// The export wrapper — `library/<family>/<port>.frames.json`'s top level.
#[derive(Deserialize, Debug, Clone)]
struct FramesExport {
    family: String,
    port: String,
    #[serde(default)]
    generated_at: Option<String>,
    #[serde(default)]
    schema_version: Option<i64>,
    #[serde(default)]
    moves: Vec<RawFrameRow>,
}

/// The measured quantities proper — everything a `RawFrameRow` carries
/// EXCEPT identity (char/move/variant/gap, the cell key) and provenance
/// (observable/method/latency/sample_n/confidence/measured_at/core_id/
/// rom_id/rig_guard_state, which live on [`FrameObservation`] instead).
/// `PartialEq` is what makes collapsing possible: two observations collapse
/// on a field exactly when their `FrameMeasurement`s agree on it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrameMeasurement {
    pub gap_px: Option<f64>,
    pub first_active_frame: Option<i64>,
    pub active: Option<i64>,
    pub recovery: Option<i64>,
    pub total: Option<i64>,
    pub hits: Option<i64>,
    pub hitstop: Option<i64>,
    pub on_hit: Option<i64>,
    pub on_block: Option<i64>,
    pub wakeup_window: Option<i64>,
    pub knockdown: Option<bool>,
    pub juggle: Option<i64>,
    pub guard_height: Option<serde_json::Value>,
    pub connect_range: Option<i64>,
    pub damage: Option<i64>,
}

impl FrameMeasurement {
    fn from_raw(r: &RawFrameRow) -> Self {
        FrameMeasurement {
            gap_px: r.gap_px,
            first_active_frame: r.first_active_frame,
            active: r.active,
            recovery: r.recovery,
            total: r.total,
            hits: r.hits,
            hitstop: r.hitstop,
            on_hit: r.on_hit,
            on_block: r.on_block,
            wakeup_window: r.wakeup_window,
            knockdown: r.knockdown.map(|v| v != 0),
            juggle: r.juggle,
            guard_height: r.guard_height.clone(),
            connect_range: r.connect_range,
            damage: r.damage,
        }
    }
}

/// One observable's contribution as `collapse_measurements` needs it: its
/// name (so a one-sided field's chosen reference can be recorded), its
/// calibrated `input_latency_frames`, and the measurement it produced.
struct CollapseInput<'a> {
    observable: &'a str,
    input_latency_frames: Option<i64>,
    raw: &'a FrameMeasurement,
}

/// Collapse a cell's per-observable measurements into one, field by field.
/// A field collapses to `Some`/`None` only when every observation agrees on
/// it — where "agrees" is field-kind-dependent (see the module-level comment
/// above: difference/anchor quantities need exact equality, one-sided
/// quantities need equality after subtracting each observation's own
/// `input_latency_frames`). A disagreement leaves that field `None` in the
/// result AND records the field's name — never picks a winner silently
/// (docs/frames.md §7, §12). Returns the collapsed measurement, the
/// disagreeing field names, and — for one-sided fields that DID collapse —
/// which observable's frame of reference the collapsed number is in.
fn collapse_measurements(
    items: &[CollapseInput],
) -> (FrameMeasurement, Vec<&'static str>, BTreeMap<&'static str, String>) {
    let mut out = FrameMeasurement::default();
    let mut disagreements = Vec::new();
    let mut one_sided_reference = BTreeMap::new();

    // Difference quantities (on_hit/on_block/hitstop, each a manifest minus
    // a manifest from the SAME observable) and anchor-based absolutes
    // (first_active_frame, active — both endpoints are reads of the same
    // anchor signal, never a probe) carry no per-observable margin, so two
    // observations must agree to the frame.
    macro_rules! exact_field {
        ($name:ident) => {{
            let mut it = items.iter().map(|o| &o.raw.$name);
            let first = it.next().expect("cell has at least one observation");
            if it.all(|v| v == first) {
                out.$name = first.clone();
            } else {
                disagreements.push(stringify!($name));
            }
        }};
    }

    // One-sided quantities (wakeup_window, total, recovery) carry the
    // measuring observable's own manifestation margin directly, so raw
    // values legitimately differ by the observables' latency delta. The
    // module comment above derives `value − input_latency_frames` as the
    // margin-independent invariant; two observations agree exactly when it
    // matches for both. Falls back to plain exact-agreement when latency
    // data is absent (older/uncalibrated exports), rather than treating
    // every such row as an automatic disagreement.
    //
    // `total`/`recovery` join `wakeup_window` here, not `exact_field!`: under
    // the only measurement protocol this project has, both are read from a
    // fixed anchor to the SAME act-again probe manifest `wakeup_window` uses
    // for a knockdown, so they carry the identical single margin (module
    // comment above, and docs/frames.md §8.4/§13 item 1).
    macro_rules! one_sided_field {
        ($name:ident) => {{
            let correctable: Vec<(i64, i64, &str)> = items
                .iter()
                .filter_map(|o| match (o.raw.$name, o.input_latency_frames) {
                    (Some(v), Some(l)) => Some((v - l, l, o.observable)),
                    _ => None,
                })
                .collect();
            if correctable.len() < items.len() {
                // Not every observation carries both a value and a latency
                // for this field — fall back to the old, unconditional
                // exact-match rule rather than guessing.
                exact_field!($name);
            } else {
                let reference_corrected = correctable[0].0;
                if correctable.iter().all(|(c, _, _)| *c == reference_corrected) {
                    // The observable with the SMALLEST latency has zero
                    // manifestation margin by the probe's construction, so
                    // its own raw reading already equals the corrected
                    // (margin-free) value — no arithmetic needed to produce
                    // the collapsed number, just picking which row to read
                    // it off of.
                    let (_, _, reference_obs) =
                        correctable.iter().min_by_key(|(_, l, _)| *l).unwrap();
                    let reference_row = items
                        .iter()
                        .find(|o| o.observable == *reference_obs)
                        .expect("reference observable is one of `items`");
                    out.$name = reference_row.raw.$name;
                    one_sided_reference.insert(stringify!($name), reference_obs.to_string());
                } else {
                    disagreements.push(stringify!($name));
                }
            }
        }};
    }

    exact_field!(gap_px);
    exact_field!(first_active_frame);
    exact_field!(active);
    exact_field!(hits);
    exact_field!(hitstop);
    exact_field!(on_hit);
    exact_field!(on_block);
    one_sided_field!(wakeup_window);
    one_sided_field!(total);
    one_sided_field!(recovery);
    exact_field!(knockdown);
    exact_field!(juggle);
    exact_field!(guard_height);
    exact_field!(connect_range);
    exact_field!(damage);
    (out, disagreements, one_sided_reference)
}

/// One observable's contribution to a cell — the provenance §12 asks for:
/// "a row without provenance is the `action_counter` mistake in another
/// costume." Never collapsed away, even when its measurement agrees with its
/// sibling observation(s) and folds into [`FrameCell::measurement`].
#[derive(Debug, Clone)]
pub struct FrameObservation {
    pub observable: String,
    pub method: String,
    pub input_latency_frames: Option<i64>,
    pub sample_n: Option<i64>,
    pub confidence: Option<String>,
    pub measured_at: Option<String>,
    pub core_id: Option<String>,
    pub rom_id: Option<String>,
    /// The rig's probe shape for this observation (§2.6/§3.1), e.g.
    /// "held+none" (attacker) or "held" (guarded defender).
    pub rig_guard_state: Option<String>,
    pub raw: FrameMeasurement,
}

/// One (char, move, variant, gap) cell — the collapse of every observable
/// row measured for it.
#[derive(Debug, Clone)]
pub struct FrameCell {
    pub char: String,
    pub move_name: String,
    pub variant: Option<String>,
    pub gap_walk_frames: Option<i64>,
    /// Collapsed measurement; a field is `None` either because nothing
    /// measured it or because the observables disagreed (see
    /// `disagreements`) — the two are NOT distinguished here on purpose:
    /// callers that care check `disagreements` explicitly rather than
    /// silently treating a disagreement as "unmeasured".
    pub measurement: FrameMeasurement,
    /// `FrameMeasurement` field names where this cell's observations
    /// disagreed. Empty on every shipped MK2 arcade cell so far (§12).
    pub disagreements: Vec<&'static str>,
    /// For each ONE-SIDED field (docs/frames.md §4.2/§8.4, e.g.
    /// `wakeup_window`) that collapsed successfully, the observable whose
    /// frame of reference `measurement`'s value is in — the one with the
    /// smallest `input_latency_frames` among this cell's observations,
    /// whose manifestation margin is zero by the probe's own construction.
    /// A bare number is ambiguous once two observables can legitimately
    /// disagree by their own latency delta; this is what removes the
    /// ambiguity. Empty for a cell with no one-sided fields, or where one
    /// disagreed (it's in `disagreements` instead, not here).
    pub one_sided_reference: BTreeMap<&'static str, String>,
    pub observations: Vec<FrameObservation>,
}

impl FrameCell {
    pub fn agrees(&self) -> bool {
        self.disagreements.is_empty()
    }

    /// The smallest per-observable `sample_n` behind this cell — the honest
    /// number for "how many independent full measurements support this",
    /// not a sum (docs/frames.md's `kit.py`: `sample_n` counts independent
    /// re-measurements, not retries).
    pub fn min_sample_n(&self) -> Option<i64> {
        self.observations.iter().filter_map(|o| o.sample_n).min()
    }
}

/// The loaded `<port>.frames.json`, queryable by character/move/gap.
#[derive(Debug, Clone)]
pub struct FrameTable {
    pub family: String,
    pub port: String,
    pub generated_at: Option<String>,
    pub schema_version: Option<i64>,
    pub cells: Vec<FrameCell>,
}

impl FrameTable {
    fn from_export(export: FramesExport) -> Result<FrameTable, String> {
        let mut groups: BTreeMap<(String, String, Option<String>, Option<i64>), Vec<RawFrameRow>> =
            BTreeMap::new();
        for row in export.moves {
            if row.family != export.family || row.port != export.port {
                return Err(format!(
                    "{}.frames.json: row family/port ('{}'/'{}') != export-level ('{}'/'{}')",
                    export.port, row.family, row.port, export.family, export.port
                ));
            }
            let key =
                (row.char.clone(), row.move_name.clone(), row.variant.clone(), row.gap_walk_frames);
            groups.entry(key).or_default().push(row);
        }
        let mut cells = Vec::new();
        for ((char, move_name, variant, gap_walk_frames), rows) in groups {
            let observations: Vec<FrameObservation> = rows
                .iter()
                .map(|r| FrameObservation {
                    observable: r.observable.clone(),
                    method: r.method.clone(),
                    input_latency_frames: r.input_latency_frames,
                    sample_n: r.sample_n,
                    confidence: r.confidence.clone(),
                    measured_at: r.measured_at.clone(),
                    core_id: r.core_id.clone(),
                    rom_id: r.rom_id.clone(),
                    rig_guard_state: r.rig_guard_state.clone(),
                    raw: FrameMeasurement::from_raw(r),
                })
                .collect();
            let collapse_inputs: Vec<CollapseInput> = observations
                .iter()
                .map(|o| CollapseInput {
                    observable: &o.observable,
                    input_latency_frames: o.input_latency_frames,
                    raw: &o.raw,
                })
                .collect();
            let (measurement, disagreements, one_sided_reference) =
                collapse_measurements(&collapse_inputs);
            cells.push(FrameCell {
                char,
                move_name,
                variant,
                gap_walk_frames,
                measurement,
                disagreements,
                one_sided_reference,
                observations,
            });
        }
        Ok(FrameTable {
            family: export.family,
            port: export.port,
            generated_at: export.generated_at,
            schema_version: export.schema_version,
            cells,
        })
    }

    pub fn chars(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.cells.iter().map(|c| c.char.as_str()).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    pub fn cells_for_char<'a>(&'a self, ch: &str) -> Vec<&'a FrameCell> {
        self.cells.iter().filter(|c| c.char == ch).collect()
    }

    pub fn moves_for_char(&self, ch: &str) -> Vec<&str> {
        let mut v: Vec<&str> =
            self.cells_for_char(ch).into_iter().map(|c| c.move_name.as_str()).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Distinct measured gaps for a character, sorted ascending (walk-frames
    /// is the reproducible fallback key per §5, so it — not `gap_px` — is
    /// the grid's column key).
    pub fn gaps_for_char(&self, ch: &str) -> Vec<i64> {
        let mut v: Vec<i64> =
            self.cells_for_char(ch).into_iter().filter_map(|c| c.gap_walk_frames).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    pub fn cell(&self, ch: &str, mv: &str, gap_walk_frames: Option<i64>) -> Option<&FrameCell> {
        self.cells
            .iter()
            .find(|c| c.char == ch && c.move_name == mv && c.gap_walk_frames == gap_walk_frames)
    }

    /// (most unsafe, safest) measured move for a character, by `on_block`.
    /// Only cells with a collapsed (non-disagreeing) `on_block` value count —
    /// "is this safe" must not be answered from a number the loader itself
    /// couldn't resolve.
    pub fn safety_extremes(&self, ch: &str) -> (Option<&FrameCell>, Option<&FrameCell>) {
        let measured: Vec<&FrameCell> = self
            .cells_for_char(ch)
            .into_iter()
            .filter(|c| c.measurement.on_block.is_some())
            .collect();
        let most_unsafe = measured.iter().min_by_key(|c| c.measurement.on_block.unwrap()).copied();
        let safest = measured.iter().max_by_key(|c| c.measurement.on_block.unwrap()).copied();
        (most_unsafe, safest)
    }
}

/// Load `<fam_dir>/<port>.frames.json` if it exists. Absence is normal and
/// silent (`Ok(None)`) — most games have no frame lab data. A PRESENT but
/// malformed file is a real error, loud like every other profile load.
fn load_frame_table(fam_dir: &Path, port: &str) -> Result<Option<FrameTable>, String> {
    let path = fam_dir.join(format!("{port}.frames.json"));
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let export: FramesExport =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    FrameTable::from_export(export).map(Some)
}

// ── the resolved profile ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GameProfile {
    pub dir: PathBuf,
    pub family: Family,
    pub port: PortProfile,
    /// `library/<family>/<port>.frames.json`, if it exists (docs/frames.md
    /// §6/§9). Optional — most games have no frame lab data, and that is
    /// normal, not a warning.
    pub frames: Option<FrameTable>,
}

impl GameProfile {
    /// Load a game profile from a path. The path may be:
    /// 1. A directory containing family.json + port profile(s):
    ///    - tries `<dirname>.profile.json` first (legacy default)
    ///    - else the single `*.profile.json` in the directory
    ///    - errors if none found or multiple without a default
    /// 2. A path like `dir/port_selector` (dir exists, file does not):
    ///    - family dir = parent
    ///    - tries `<parent>/<port_selector>.profile.json`
    ///    - else scans for a profile with `"port": "<port_selector>"`
    ///    - exactly one match wins; else error
    pub fn load(dir: &Path) -> Result<GameProfile, String> {
        // Determine family dir and profile path.
        let (fam_dir, prof_path) = Self::resolve_game_dir(dir)?;

        let fam_path = fam_dir.join("family.json");
        let family: Family = serde_json::from_str(
            &std::fs::read_to_string(&fam_path)
                .map_err(|e| format!("{}: {e}", fam_path.display()))?,
        )
        .map_err(|e| format!("{}: {e}", fam_path.display()))?;

        let port: PortProfile = serde_json::from_str(
            &std::fs::read_to_string(&prof_path)
                .map_err(|e| format!("{}: {e}", prof_path.display()))?,
        )
        .map_err(|e| format!("{}: {e}", prof_path.display()))?;

        if port.family != family.family {
            return Err(format!(
                "profile family '{}' != family.json '{}'",
                port.family, family.family
            ));
        }

        // Validate record_globals names exist in globals.
        for rg in &port.memory.record_globals {
            if !port.memory.globals.contains_key(&rg.name) {
                return Err(format!("record_globals names unknown global '{}'", rg.name));
            }
        }

        // Validate a Located timer_hold: the scan window must be non-empty,
        // there must be at least one write, and every predicate must have a
        // readable width and EXACTLY ONE predicate kind. Every `eq_global`
        // must name a declared global (a typo fails the load, not silently
        // declines the feature — the legacy Adjacent form keeps its
        // lookup-by-convention `round_timer` soft-decline unchanged).
        if let TimerHold::Located { scan, record, writes } = &port.enforcement.timer_hold {
            if scan[0].0 >= scan[1].0 {
                return Err(format!(
                    "enforcement.timer_hold scan start {:#x} is not < end {:#x}",
                    scan[0].0, scan[1].0
                ));
            }
            if writes.is_empty() {
                return Err("enforcement.timer_hold located form has no writes".into());
            }
            for pr in record {
                if !matches!(pr.size, 1 | 2 | 4) {
                    return Err(format!(
                        "enforcement.timer_hold predicate size {} is not 1/2/4",
                        pr.size
                    ));
                }
                // A range needs BOTH bounds; count kinds as equals / range /
                // eq_global and require exactly one.
                if pr.min.is_some() != pr.max.is_some() {
                    return Err(format!(
                        "enforcement.timer_hold range predicate at offset {:#x} needs both min and max",
                        pr.offset.0
                    ));
                }
                let kinds = pr.equals.is_some() as u8
                    + (pr.min.is_some() && pr.max.is_some()) as u8
                    + pr.eq_global.is_some() as u8;
                if kinds != 1 {
                    return Err(format!(
                        "enforcement.timer_hold predicate at offset {:#x} must have exactly one of equals / min+max / eq_global",
                        pr.offset.0
                    ));
                }
                if let Some(name) = &pr.eq_global {
                    if !port.memory.globals.contains_key(name) {
                        return Err(format!(
                            "enforcement.timer_hold names unknown global '{name}'"
                        ));
                    }
                }
            }
        }

        // Validate hitstun_sources names appear in the recorded-globals union.
        if let Some(hs) = &port.hitstun_sources {
            let recorded_names: Vec<&str> = port.gate
                .iter()
                .filter_map(|c| c.global_name())
                .chain(port.memory.record_globals.iter().map(|rg| rg.name.as_str()))
                .collect();
            for global_name in hs.values() {
                if !recorded_names.iter().any(|n| *n == global_name) {
                    return Err(format!(
                        "hitstun_sources names unrecorded global '{}'",
                        global_name
                    ));
                }
            }
        }

        // Validate fighter fields: exactly one source, and globals/object_ptr
        // resolve.
        for f in &port.memory.fighter_fields {
            match f.via.as_deref() {
                Some("object_ptr") => {
                    if f.globals.is_some() {
                        return Err(format!(
                            "fighter field '{}' has both via=object_ptr and globals — pick one",
                            f.name
                        ));
                    }
                    if f.off.is_none() {
                        return Err(format!(
                            "fighter field '{}' has via=object_ptr but no off \
                             (offset from the decoded object)",
                            f.name
                        ));
                    }
                    if port.memory.blocks.object_ptr.is_none() {
                        return Err(format!(
                            "fighter field '{}' uses via=object_ptr but \
                             memory.blocks.object_ptr is not declared",
                            f.name
                        ));
                    }
                }
                Some(other) => {
                    return Err(format!(
                        "fighter field '{}' names unknown via '{other}' \
                         (only 'object_ptr' is supported)",
                        f.name
                    ));
                }
                None => match (&f.off, &f.globals) {
                    (Some(_), Some(_)) => {
                        return Err(format!(
                            "fighter field '{}' has both off and globals — pick one",
                            f.name
                        ));
                    }
                    (None, None) => {
                        return Err(format!(
                            "fighter field '{}' needs off or globals",
                            f.name
                        ));
                    }
                    (None, Some(g)) => {
                        for gname in [&g.block1, &g.block2] {
                            if !port.memory.globals.contains_key(gname) {
                                return Err(format!(
                                    "fighter field '{}' names unknown global '{gname}'",
                                    f.name
                                ));
                            }
                        }
                    }
                    (Some(_), None) => {}
                },
            }
        }

        // Validate pin globals resolve.
        for pin in &port.pins {
            if !port.memory.globals.contains_key(&pin.global) {
                return Err(format!("pins names unknown global '{}'", pin.global));
            }
        }

        // Validate id_map values exist in family roster.
        if let Some(im) = &port.id_map {
            for canonical_id in im.values() {
                if !family.roster.iter().any(|r| r.id == *canonical_id) {
                    return Err(format!("id_map maps to unknown roster id {}", canonical_id));
                }
            }
        }

        // Every gate/stage global must resolve; every chord class must exist.
        for cond in &port.gate {
            if let Some(g) = cond.global_name() {
                if !port.memory.globals.contains_key(g) {
                    return Err(format!("gate condition names unknown global '{g}'"));
                }
            }
        }
        for class in port.attack_chords.keys() {
            if !family.attack_classes.iter().any(|c| c == class) {
                return Err(format!("attack_chords names unknown class '{class}'"));
            }
        }

        // Macro-action vocabulary (MACRO_ACTIONS §1/§2): moves are keyed by
        // roster NAME; encodings must reference declared moves, chord-backed
        // classes, and dirs from the closed semantic set.
        for chr in family.moves.keys() {
            if !family.roster.iter().any(|r| r.name == *chr) {
                return Err(format!("moves names unknown roster character '{chr}'"));
            }
        }
        const DIRS: [&str; 4] = ["back", "forward", "up", "down"];
        for (chr, mvs) in &port.special_inputs {
            let Some(fam_moves) = family.moves.get(chr) else {
                return Err(format!("special_inputs names character '{chr}' absent from family moves"));
            };
            for (mv, steps) in mvs {
                if !fam_moves.iter().any(|m| m.name == *mv) {
                    return Err(format!("special_inputs '{chr}' encodes unknown move '{mv}'"));
                }
                if steps.is_empty() {
                    return Err(format!("special_inputs '{chr}.{mv}' has no steps"));
                }
                for st in steps {
                    if st.dirs.is_empty()
                        && st.press.is_empty()
                        && st.hold.is_empty()
                        && st.release.is_empty()
                    {
                        return Err(format!("special_inputs '{chr}.{mv}' has an empty step"));
                    }
                    // §10.1: press/hold/release are mutually exclusive — they
                    // name the step's KIND (Normal/Hold/Release).
                    let kinds = [!st.press.is_empty(), !st.hold.is_empty(), !st.release.is_empty()];
                    if kinds.iter().filter(|k| **k).count() > 1 {
                        return Err(format!(
                            "special_inputs '{chr}.{mv}' step mixes press/hold/release — pick one"
                        ));
                    }
                    if !st.hold.is_empty() {
                        match st.min_frames {
                            Some(mf) if mf > 0 => {}
                            _ => {
                                return Err(format!(
                                    "special_inputs '{chr}.{mv}' hold step needs a positive min_frames"
                                ));
                            }
                        }
                    } else if st.min_frames.is_some() {
                        return Err(format!(
                            "special_inputs '{chr}.{mv}' min_frames set without a hold step"
                        ));
                    }
                    for d in &st.dirs {
                        if !DIRS.contains(&d.as_str()) {
                            return Err(format!("special_inputs '{chr}.{mv}' names unknown dir '{d}'"));
                        }
                    }
                    for class in st.press.iter().chain(&st.hold).chain(&st.release).chain(&st.while_held) {
                        if !port.attack_chords.contains_key(class) {
                            return Err(format!(
                                "special_inputs '{chr}.{mv}' names unknown class '{class}'"
                            ));
                        }
                    }
                }
            }
        }
        if let Some(cs) = &port.contact_signal {
            match (&cs.field, &cs.global) {
                (Some(_), Some(_)) => {
                    return Err("contact_signal: pick field OR global, not both".to_string());
                }
                (None, None) => {
                    return Err("contact_signal needs field or global".to_string());
                }
                (Some(f), None) => {
                    if !port.memory.fighter_fields.iter().any(|x| &x.name == f) {
                        return Err(format!("contact_signal names unknown field '{f}'"));
                    }
                }
                (None, Some(gl)) => {
                    if !port.memory.globals.contains_key(gl) {
                        return Err(format!("contact_signal names unknown global '{gl}'"));
                    }
                }
            }
            if let Some(d) = &cs.direction {
                if d != "decrease" {
                    return Err(format!(
                        "contact_signal.direction must be \"decrease\" or absent (got '{d}')"
                    ));
                }
            }
        }
        let frames = load_frame_table(&fam_dir, &port.port)?;

        Ok(GameProfile { dir: fam_dir, family, port, frames })
    }

    /// Resolve a game path to (family_dir, profile_path).
    ///
    /// Handles both directory and port-selector cases per the §5.2 contract:
    /// 1. `dir` is a directory → family dir = dir; profile = <dirname>.profile.json or single *.profile.json
    /// 2. `dir` is not a directory but parent is → family dir = parent; selector = basename;
    ///    try <parent>/<basename>.profile.json, then scan for matching "port" field
    /// 3. Neither → error
    fn resolve_game_dir(input: &Path) -> Result<(PathBuf, PathBuf), String> {
        if input.is_dir() {
            // Case 1: dir is a directory.
            let fam_dir = input.to_path_buf();
            let stem = fam_dir
                .file_name()
                .map(|s| s.to_string_lossy().into_owned());

            // Try <dirname>.profile.json first.
            let default_path = stem
                .as_deref()
                .map(|s| fam_dir.join(format!("{s}.profile.json")));
            if let Some(path) = default_path {
                if path.is_file() {
                    return Ok((fam_dir, path));
                }
            }

            // Try single *.profile.json.
            let profiles: Vec<PathBuf> = std::fs::read_dir(&fam_dir)
                .ok()
                .and_then(|entries| {
                    let found: Vec<PathBuf> = entries
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| p.to_string_lossy().ends_with(".profile.json"))
                        .collect();
                    if found.is_empty() { None } else { Some(found) }
                })
                .unwrap_or_default();

            if profiles.is_empty() {
                return Err(format!("{}: no *.profile.json found", fam_dir.display()));
            }
            if profiles.len() == 1 {
                return Ok((fam_dir, profiles[0].clone()));
            }

            // Multiple profiles, no default → error with suggestion.
            let stems: Vec<String> = profiles
                .iter()
                .filter_map(|p| {
                    // file_stem on "mk2.profile.json" is "mk2.profile" — trim
                    // the ".profile" so the suggestion names the port segment.
                    p.file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.trim_end_matches(".profile").to_string())
                })
                .collect();
            return Err(format!(
                "{}: multiple port profiles and no {}.profile.json default — select one: --game {}/{}",
                fam_dir.display(),
                stem.as_deref().unwrap_or(""),
                fam_dir.display(),
                stems.join("|")
            ));
        }

        // Case 2: not a directory; check if parent is a directory.
        if let Some(parent) = input.parent() {
            if parent.is_dir() {
                let fam_dir = parent.to_path_buf();
                let selector = input
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .ok_or_else(|| format!("--game {}: invalid path", input.display()))?;

                // Try <parent>/<selector>.profile.json.
                let default_path = fam_dir.join(format!("{}.profile.json", selector));
                if default_path.is_file() {
                    return Ok((fam_dir, default_path));
                }

                // Scan for matching "port" field.
                let profiles: Vec<(PathBuf, String)> = std::fs::read_dir(&fam_dir)
                    .ok()
                    .and_then(|entries| {
                        let mut matches = Vec::new();
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.to_string_lossy().ends_with(".profile.json") {
                                if let Ok(content) = std::fs::read_to_string(&path) {
                                    if let Ok(obj) =
                                        serde_json::from_str::<serde_json::Value>(&content)
                                    {
                                        if let Some(port_val) = obj.get("port") {
                                            if let Some(port_str) = port_val.as_str() {
                                                if port_str == selector {
                                                    matches.push((
                                                        path.clone(),
                                                        port_str.to_string(),
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if matches.is_empty() {
                            None
                        } else {
                            Some(matches)
                        }
                    })
                    .unwrap_or_default();

                if profiles.is_empty() {
                    // Collect available profiles for the error message.
                    let available: Vec<String> = std::fs::read_dir(&fam_dir)
                        .ok()
                        .and_then(|entries| {
                            let mut stems = Vec::new();
                            let mut ports = Vec::new();
                            for entry in entries.flatten() {
                                let path = entry.path();
                                if path.to_string_lossy().ends_with(".profile.json") {
                                    if let Some(stem) = path.file_stem() {
                                        stems.push(
                                            stem.to_string_lossy()
                                                .trim_end_matches(".profile")
                                                .to_string(),
                                        );
                                    }
                                    if let Ok(content) = std::fs::read_to_string(&path) {
                                        if let Ok(obj) = serde_json::from_str::<serde_json::Value>(
                                            &content,
                                        ) {
                                            if let Some(port_val) = obj.get("port") {
                                                if let Some(port_str) = port_val.as_str() {
                                                    ports.push(port_str.to_string());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            stems.sort();
                            ports.sort();
                            stems.extend(ports);
                            if stems.is_empty() {
                                None
                            } else {
                                Some(stems)
                            }
                        })
                        .unwrap_or_default();

                    let available_str = if available.is_empty() {
                        "none".to_string()
                    } else {
                        available.join("/")
                    };

                    return Err(format!(
                        "{}: no port '{}' (no {}.profile.json and no profile with \"port\": \"{}\"); available: {}",
                        fam_dir.display(), selector, selector, selector, available_str
                    ));
                }

                if profiles.len() == 1 {
                    return Ok((fam_dir, profiles[0].0.clone()));
                }

                // Ambiguous: multiple matches.
                let files: Vec<String> = profiles
                    .iter()
                    .map(|(p, _)| p.file_name().unwrap().to_string_lossy().to_string())
                    .collect();
                return Err(format!(
                    "{}: port '{}' is ambiguous: {}",
                    fam_dir.display(),
                    selector,
                    files.join(", ")
                ));
            }
        }

        // Case 3: neither conditions met.
        Err(format!("--game {}: no such game directory", input.display()))
    }

    // ── convenience accessors (the API call sites use) ──────────────────

    pub fn global(&self, name: &str) -> Option<u32> {
        self.port.memory.globals.get(name).map(|a| a.0)
    }

    pub fn block1(&self) -> u32 {
        self.port.memory.blocks.block1.0
    }

    pub fn block2(&self) -> u32 {
        self.port.memory.blocks.block2.0
    }

    /// Offset + size for an OFFSET-based fighter field. Returns None for
    /// global-sourced fields (callers that need those use [`field_addr`],
    /// or fall back to the per-player globals themselves — training's
    /// x_pair pattern) and for `via: "object_ptr"` fields (no fixed address
    /// at all — see [`object_ptr_field`](Self::object_ptr_field)).
    pub fn field_off(&self, name: &str) -> Option<(u32, u8)> {
        self.port
            .memory
            .fighter_fields
            .iter()
            .find(|f| f.name == name)
            .filter(|f| f.via.is_none())
            .and_then(|f| f.off.as_ref().map(|o| (o.0, f.size)))
    }

    /// ABSOLUTE address + size of a fighter field for one block (1 or 2),
    /// resolving both STATIC variants: block base + offset, or the per-block
    /// global. Returns `None` for `via: "object_ptr"` fields — those have no
    /// fixed address (the object pool slot moves every frame); read them
    /// live with [`object_ptr_field`](Self::object_ptr_field) instead.
    pub fn field_addr(&self, block: u8, name: &str) -> Option<(u32, u8)> {
        let f = self.port.memory.fighter_fields.iter().find(|f| f.name == name)?;
        if f.via.is_some() {
            return None;
        }
        let base = if block == 1 { self.block1() } else { self.block2() };
        if let Some(off) = &f.off {
            return Some((base.wrapping_add(off.0), f.size));
        }
        let g = f.globals.as_ref()?;
        let gname = if block == 1 { &g.block1 } else { &g.block2 };
        Some((self.global(gname)?, f.size))
    }

    /// Whether fighter field `name` is pointer-resolved (`via: "object_ptr"`).
    pub fn field_is_object_ptr(&self, name: &str) -> bool {
        self.port
            .memory
            .fighter_fields
            .iter()
            .any(|f| f.name == name && f.via.as_deref() == Some("object_ptr"))
    }

    /// Live-read a `via: "object_ptr"` fighter field for one block (1 or 2)
    /// through a caller-supplied reader: `read(addr, size_bytes) -> natural
    /// value` — endianness already resolved by the caller (training mode's
    /// `rd8`/`rd16`-shaped helpers are exactly this shape; profile.rs stays
    /// endianness-agnostic here).
    ///
    /// Returns `None` — ABSENT, never a synthesized 0 (RECORDER_V3 law,
    /// docs/frames.md §2.5) — when: `name` isn't a `via: "object_ptr"`
    /// field, no `object_ptr` is declared for this port, the raw pointer
    /// word falls outside its `valid_range` (not live this frame), or the
    /// char-id cross-check at `obj + cid_check_off` disagrees with the
    /// fighter block's own char_id (the pointer went stale — the pool slot
    /// was reused by a different object, docs/frames.md §5).
    pub fn object_ptr_field(
        &self,
        block: u8,
        name: &str,
        mut read: impl FnMut(u32, u8) -> u32,
    ) -> Option<i64> {
        let f = self.port.memory.fighter_fields.iter().find(|f| f.name == name)?;
        if f.via.as_deref() != Some("object_ptr") {
            return None;
        }
        let obj_ptr = self.port.memory.blocks.object_ptr.as_ref()?;
        let field_off = f.off.as_ref()?.0;
        let base = if block == 1 { self.block1() } else { self.block2() };

        let ptr_addr = base.wrapping_add_signed(obj_ptr.off.0);
        let raw_ptr = read(ptr_addr, obj_ptr.size);
        let obj = obj_ptr.decode(raw_ptr)?;

        // Staleness cross-check: obj+cid_check_off MUST equal block+0.
        let cid_at_obj = read(obj.wrapping_add(obj_ptr.cid_check_off.0), 1);
        let cid_at_block = read(base, 1);
        if cid_at_obj != cid_at_block {
            return None;
        }

        let raw = read(obj.wrapping_add(field_off), f.size);
        Some(if f.signed { sign_extend(raw, f.size) } else { raw as i64 })
    }

    pub fn char_name(&self, id: u8) -> String {
        self.family
            .roster
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| format!("c{id}"))
    }

    pub fn matchup_slug(&self, me: u8, opp: u8) -> String {
        format!("{}-vs-{}", self.char_name(me), self.char_name(opp))
    }

    /// Selector value to freeze to fight `opp` next (None if no value).
    pub fn stage_value_for_opponent(&self, opp: u8) -> Option<u8> {
        let ss = self.port.stage_select.as_ref()?;
        ss.value_to_home_char
            .iter()
            .find(|(_, home)| **home == opp)
            .and_then(|(v, _)| v.parse().ok())
    }

    pub fn opponent_for_stage_value(&self, v: u8) -> Option<u8> {
        let ss = self.port.stage_select.as_ref()?;
        ss.value_to_home_char.get(&v.to_string()).copied()
    }

    pub fn calibration(&self, key: &str) -> Option<f64> {
        self.port.calibration.get(key).copied()
    }

    /// Session pins resolved to (address, value) pairs — profile load
    /// guarantees every pin global resolves.
    pub fn resolved_pins(&self) -> Vec<(u32, u8)> {
        self.port
            .pins
            .iter()
            .filter_map(|p| Some((self.global(&p.global)?, p.value)))
            .collect()
    }

    /// The family∩port special intersection for a CANONICAL char id — the
    /// legal offer list (panel, executor): family `moves` order, only the
    /// moves this port encodes. Empty when either side omits the character.
    pub fn specials_for(&self, canon_id: u8) -> Vec<(&str, &[StepSpec])> {
        let Some(name) = self.family.roster.iter().find(|r| r.id == canon_id).map(|r| &r.name)
        else {
            return Vec::new();
        };
        let (Some(fam), Some(enc)) =
            (self.family.moves.get(name), self.port.special_inputs.get(name))
        else {
            return Vec::new();
        };
        fam.iter()
            .filter_map(|m| enc.get(&m.name).map(|s| (m.name.as_str(), s.as_slice())))
            .collect()
    }

    /// Every encoding this port carries (family∩port across all characters),
    /// in deterministic (character, family-move) order — the recorder's
    /// char-blind matcher input. Duplicate move names across characters are
    /// deduped by the matcher at emission time, not here.
    pub fn all_specials(&self) -> Vec<(&str, &[StepSpec])> {
        self.port
            .special_inputs
            .iter()
            .filter_map(|(chr, enc)| self.family.moves.get(chr).map(|fam| (fam, enc)))
            .flat_map(|(fam, enc)| {
                fam.iter()
                    .filter_map(|m| enc.get(&m.name).map(|s| (m.name.as_str(), s.as_slice())))
            })
            .collect()
    }

    /// Translate a raw RAM char id to its canonical roster id.
    /// If no id_map is present or the raw id is not in the map, returns identity (raw).
    #[allow(dead_code)]
    pub fn canon_char_id(&self, raw: u8) -> u8 {
        self.port
            .id_map
            .as_ref()
            .and_then(|m| m.get(&raw.to_string()).copied())
            .unwrap_or(raw)
    }
}

/// Sign-extend a raw, already-natural-order value of `size` bytes (1 or 2)
/// to `i64`, per a fighter field's `signed: true` declaration. Widths other
/// than 1/2 pass through unsigned (the schema only uses this for 8/16-bit
/// fighter fields).
fn sign_extend(raw: u32, size: u8) -> i64 {
    match size {
        1 => raw as u8 as i8 as i64,
        2 => raw as u16 as i16 as i64,
        _ => raw as i64,
    }
}

/// RETRO joypad bit for a button name as used in `attack_chords` (the
/// RETRO_DEVICE_ID order every mask in the codebase shares).
pub fn retro_button_bit(name: &str) -> Option<u16> {
    Some(match name {
        "b" => 0,
        "y" => 1,
        "select" => 2,
        "start" => 3,
        "up" => 4,
        "down" => 5,
        "left" => 6,
        "right" => 7,
        "a" => 8,
        "x" => 9,
        "l" => 10,
        "r" => 11,
        _ => return None,
    })
}

impl GateCond {
    pub fn global_name(&self) -> Option<&str> {
        match self {
            GateCond::ByteZero { global }
            | GateCond::WordZero { global }
            | GateCond::BcdValidNonzero { global }
            | GateCond::WordMaskedNotAll { global, .. } => Some(global),
            GateCond::HealthInRange { .. } => None,
        }
    }
}

// ── process-wide instance ───────────────────────────────────────────────────

static CURRENT: OnceLock<GameProfile> = OnceLock::new();

/// Load and install the process profile. Call once at startup, before any
/// consumer; a second call is an error (one game per process by design).
pub fn init(dir: &Path) -> Result<&'static GameProfile, String> {
    let p = GameProfile::load(dir)?;
    CURRENT
        .set(p)
        .map_err(|_| "profile::init called twice".to_string())?;
    Ok(CURRENT.get().unwrap())
}

/// The loaded profile. Panics if `init` has not run — that is a startup
/// wiring bug, not a runtime condition. Tests use `init_for_tests`.
pub fn current() -> &'static GameProfile {
    CURRENT.get().expect("profile::init not called")
}

/// Test helper: install the asurabld profile if nothing is loaded yet
/// (idempotent — safe under the multi-threaded test runner).
#[cfg(test)]
pub fn init_for_tests() -> &'static GameProfile {
    if CURRENT.get().is_none() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("library/asurabld");
        let _ = CURRENT.set(GameProfile::load(&dir).expect("asurabld profile parses"));
    }
    CURRENT.get().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Create a unique temp directory path for tests. The caller is responsible for cleanup.
    fn make_test_dir(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join("rustretro_tests");
        let _ = fs::create_dir_all(&base);
        let path = base.join(format!(
            "{}_{}_{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&path); // Clean up if it exists
        fs::create_dir_all(&path).ok();
        path
    }

    #[test]
    fn shipped_asurabld_profile_parses_and_matches_the_old_constants() {
        let p = init_for_tests();
        assert_eq!(p.family.family, "asurabld");
        assert_eq!(p.port.port, "arcade");
        // The values the compiled constants used to hold.
        assert_eq!(p.block1(), 0x403798);
        assert_eq!(p.block2(), 0x40454C);
        assert_eq!(p.port.memory.blocks.stride.0, 0xDB4);
        assert_eq!(p.global("round_timer"), Some(0x40000A));
        assert_eq!(p.global("char_select"), Some(0x400006));
        assert_eq!(p.global("credits"), Some(0x40655D));
        assert_eq!(p.field_off("health"), Some((0x177, 1)));
        assert_eq!(p.field_off("char_id"), Some((0x639, 1)));
        assert_eq!(p.char_name(1), "goat");
        assert_eq!(p.char_name(9), "sgeist");
        assert_eq!(p.char_name(11), "c11");
        assert_eq!(p.matchup_slug(1, 7), "goat-vs-rosemary");
        // Stage selector round-trips like record.rs's tables.
        assert_eq!(p.stage_value_for_opponent(7), Some(5));
        assert_eq!(p.opponent_for_stage_value(9), Some(9));
        assert_eq!(p.stage_value_for_opponent(3), None); // footee
        // Gate: six conditions, v3 (char_select present).
        assert_eq!(p.port.gate.len(), 6);
        assert!(p.port.gate.iter().any(|c| c.global_name() == Some("char_select")));
        // Chords cover every non-None attack class.
        for class in p.family.attack_classes.iter().filter(|c| *c != "None") {
            assert!(p.port.attack_chords.contains_key(class), "{class} chord missing");
        }
        assert_eq!(p.port.enforcement.health_max, 0xEF);
        assert_eq!(p.port.enforcement.timer_hold, TimerHold::Adjacent([0x85, 0x03]));
        assert_eq!(p.calibration("GROUND_Y"), Some(216.0));
    }

    #[test]
    fn path_resolution_default_directory_with_matching_stem() {
        // Test case 1: dir exists, <dirname>.profile.json exists → use it
        let tmpbase = make_test_dir("path_resolution_default");
        let game_dir = tmpbase.join("mygame");
        fs::create_dir(&game_dir).unwrap();

        // Create family.json
        let family_json = r#"{"family":"test","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        // Create mygame.profile.json (default via stem)
        let port_json = r#"{"family":"test","port":"default","core":{"library_name":"","provenance_game":"test","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("mygame.profile.json"), port_json).unwrap();

        let result = GameProfile::load(&game_dir);
        assert!(result.is_ok());
        let profile = result.unwrap();
        assert_eq!(profile.family.family, "test");
        assert_eq!(profile.port.port, "default");
    }

    #[test]
    fn path_resolution_single_profile_fallback() {
        // Test case 1b: dir exists, no <dirname>.profile.json, single *.profile.json → use it
        let tmpbase = make_test_dir("path_resolution_single");
        let game_dir = tmpbase.join("gameX");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"test2","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        // Only one profile with a different name
        let port_json = r#"{"family":"test2","port":"only","core":{"library_name":"","provenance_game":"test2","provenance_core":"test2"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("other.profile.json"), port_json).unwrap();

        let result = GameProfile::load(&game_dir);
        assert!(result.is_ok());
        let profile = result.unwrap();
        assert_eq!(profile.port.port, "only");
    }

    #[test]
    fn path_resolution_multiple_profiles_error() {
        // Test case 1c: dir exists, multiple *.profile.json, no default → error
        let tmpbase = make_test_dir("path_resolution_multiple");
        let game_dir = tmpbase.join("ambiguous");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"test3","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json_a = r#"{"family":"test3","port":"arcade","core":{"library_name":"","provenance_game":"test3","provenance_core":"test3"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("arcade.profile.json"), port_json_a).unwrap();

        let port_json_g = r#"{"family":"test3","port":"genesis","core":{"library_name":"","provenance_game":"test3","provenance_core":"test3"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("genesis.profile.json"), port_json_g).unwrap();

        let result = GameProfile::load(&game_dir);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("multiple port profiles"));
        assert!(err.contains("arcade") || err.contains("genesis"));
    }

    #[test]
    fn path_resolution_port_segment_by_filename() {
        // Test case 2a: dir/selector path where selector.profile.json exists
        let tmpbase = make_test_dir("path_resolution_port_segment");
        let game_dir = tmpbase.join("mk2");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"mk2","roster":[{"id":0,"name":"test"}],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json_mk2 = r#"{"family":"mk2","port":"arcade","core":{"library_name":"","provenance_game":"mk2","provenance_core":"fbneo"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":161,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("mk2.profile.json"), port_json_mk2).unwrap();

        let port_json_gen = r#"{"family":"mk2","port":"genesis","core":{"library_name":"","provenance_game":"mk2","provenance_core":"genesis_plus"},"memory":{"blocks":{"block1":"0xFF8000","block2":"0xFF8200","stride":"0x200"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":161,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("genesis.profile.json"), port_json_gen)
            .unwrap();

        // Load via selector path
        let selector_path = tmpbase.join("mk2/genesis");
        let result = GameProfile::load(&selector_path);
        assert!(result.is_ok());
        let profile = result.unwrap();
        assert_eq!(profile.port.port, "genesis");
    }

    #[test]
    fn path_resolution_port_segment_by_field_match() {
        // Test case 2b: dir/selector where selector matches a "port" field value
        let tmpbase = make_test_dir("path_resolution_port_field");
        let game_dir = tmpbase.join("mk2");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"mk2","roster":[{"id":0,"name":"test"}],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        // File named differently but port="arcade"
        let port_json_default = r#"{"family":"mk2","port":"arcade","core":{"library_name":"","provenance_game":"mk2","provenance_core":"fbneo"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":161,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("mk2.profile.json"), port_json_default).unwrap();

        // File named port_v2 but port="v2"
        let port_json_v2 = r#"{"family":"mk2","port":"v2","core":{"library_name":"","provenance_game":"mk2","provenance_core":"fbneo"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":161,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("port_v2.profile.json"), port_json_v2).unwrap();

        // Try to load via --game mk2/v2 (should match the port field, not filename)
        let selector_path = tmpbase.join("mk2/v2");
        let result = GameProfile::load(&selector_path);
        assert!(result.is_ok());
        let profile = result.unwrap();
        assert_eq!(profile.port.port, "v2");
    }

    #[test]
    fn path_resolution_port_segment_ambiguous_error() {
        // Test case 2d: multiple profiles with the same port field value → error
        let tmpbase = make_test_dir("path_resolution_ambiguous");
        let game_dir = tmpbase.join("bad");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"bad","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json_1 = r#"{"family":"bad","port":"dup","core":{"library_name":"","provenance_game":"bad","provenance_core":"bad"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("first.profile.json"), port_json_1).unwrap();

        let port_json_2 = r#"{"family":"bad","port":"dup","core":{"library_name":"","provenance_game":"bad","provenance_core":"bad"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("second.profile.json"), port_json_2).unwrap();

        let selector_path = tmpbase.join("bad/dup");
        let result = GameProfile::load(&selector_path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("ambiguous"));
    }

    #[test]
    fn path_resolution_port_segment_not_found_error() {
        // Test case 2c: selector doesn't match any profile → error
        let tmpbase = make_test_dir("path_resolution_not_found");
        let game_dir = tmpbase.join("mk2");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"mk2","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"mk2","port":"arcade","core":{"library_name":"","provenance_game":"mk2","provenance_core":"fbneo"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":161,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("mk2.profile.json"), port_json).unwrap();

        let selector_path = tmpbase.join("mk2/nonexistent");
        let result = GameProfile::load(&selector_path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("no port 'nonexistent'"));
    }

    #[test]
    fn id_map_present_and_mapped() {
        // canon_char_id with a present id_map entry
        let tmpbase = make_test_dir("id_map_present");
        let game_dir = tmpbase.join("mapped");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"mapped","roster":[{"id":0,"name":"a"},{"id":1,"name":"b"},{"id":2,"name":"c"}],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"mapped","port":"test","core":{"library_name":"","provenance_game":"mapped","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{},"id_map":{"5":0,"6":1,"7":2}}"#;
        fs::write(game_dir.join("mapped.profile.json"), port_json).unwrap();

        let profile = GameProfile::load(&game_dir).unwrap();
        assert_eq!(profile.canon_char_id(5), 0);
        assert_eq!(profile.canon_char_id(6), 1);
        assert_eq!(profile.canon_char_id(7), 2);
    }

    #[test]
    fn id_map_absent_uses_identity() {
        // canon_char_id with no id_map → identity
        let tmpbase = make_test_dir("id_map_absent");
        let game_dir = tmpbase.join("nomapped");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"nomapped","roster":[{"id":0,"name":"a"},{"id":5,"name":"b"}],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"nomapped","port":"test","core":{"library_name":"","provenance_game":"nomapped","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("nomapped.profile.json"), port_json).unwrap();

        let profile = GameProfile::load(&game_dir).unwrap();
        assert_eq!(profile.canon_char_id(5), 5);
        assert_eq!(profile.canon_char_id(0), 0);
    }

    #[test]
    fn id_map_unmapped_key_uses_identity() {
        // canon_char_id with id_map present but key missing → identity
        let tmpbase = make_test_dir("id_map_unmapped");
        let game_dir = tmpbase.join("partial");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"partial","roster":[{"id":0,"name":"a"},{"id":1,"name":"b"},{"id":5,"name":"c"}],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"partial","port":"test","core":{"library_name":"","provenance_game":"partial","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{},"id_map":{"5":0}}"#;
        fs::write(game_dir.join("partial.profile.json"), port_json).unwrap();

        let profile = GameProfile::load(&game_dir).unwrap();
        assert_eq!(profile.canon_char_id(5), 0); // mapped
        assert_eq!(profile.canon_char_id(99), 99); // unmapped → identity
    }

    #[test]
    fn record_globals_valid() {
        // record_globals with valid globals
        let tmpbase = make_test_dir("record_globals_valid");
        let game_dir = tmpbase.join("recorded");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"recorded","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"recorded","port":"test","core":{"library_name":"","provenance_game":"recorded","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{"combo":"0x1000","demo":"0x2000"},"record_globals":[{"name":"combo","size":1},{"name":"demo","size":2}]},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("recorded.profile.json"), port_json).unwrap();

        let result = GameProfile::load(&game_dir);
        assert!(result.is_ok());
    }

    #[test]
    fn record_globals_invalid_global_name() {
        // record_globals with an unknown global → error
        let tmpbase = make_test_dir("record_globals_invalid");
        let game_dir = tmpbase.join("badrecord");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"badrecord","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"badrecord","port":"test","core":{"library_name":"","provenance_game":"badrecord","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{"known":"0x1000"},"record_globals":[{"name":"unknown","size":1}]},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("badrecord.profile.json"), port_json).unwrap();

        let result = GameProfile::load(&game_dir);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("record_globals names unknown global"));
    }

    /// The Located timer_hold form (mk2.md "The round timer, closed"): parses
    /// into `TimerHold::Located`, and load-validation rejects each malformed
    /// shape loudly (bad size, two predicate kinds, unknown eq_global global,
    /// empty writes, empty scan) rather than silently declining the feature.
    #[test]
    fn timer_hold_located_parses_and_validates() {
        let tmpbase = make_test_dir("timer_hold_located");
        let game_dir = tmpbase.join("located");
        fs::create_dir(&game_dir).unwrap();
        let family_json = r#"{"family":"located","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let mk = |timer_hold: &str| {
            format!(
                r#"{{"family":"located","port":"test","core":{{"library_name":"","provenance_game":"located","provenance_core":"test"}},"memory":{{"blocks":{{"block1":"0x0","block2":"0x0","stride":"0x0"}},"fighter_fields":[],"globals":{{"timer_tens":"0xBD74","timer_ones":"0xBD76"}}}},"gate":[],"enforcement":{{"health_max":255,"refill_below":1,"timer_hold":{timer_hold},"credits_target":0,"credits_min":0}},"calibration":{{}},"attack_chords":{{}}}}"#
            )
        };
        let good = r#"{"scan":["0xC000","0xF000"],"record":[{"offset":"0xE","size":4,"equals":"0x0000000B"},{"offset":"0x2","size":4,"min":"0x01060000","max":"0x01080000"},{"offset":"0x6","size":1,"eq_global":"timer_tens"},{"offset":"0xA","size":1,"eq_global":"timer_ones"}],"writes":[{"offset":"0x6","value":9},{"offset":"0xA","value":9}]}"#;
        fs::write(game_dir.join("located.profile.json"), mk(good)).unwrap();
        let p = GameProfile::load(&game_dir).unwrap();
        match &p.port.enforcement.timer_hold {
            TimerHold::Located { scan, record, writes } => {
                assert_eq!(*scan, [HexAddr(0xC000), HexAddr(0xF000)]);
                assert_eq!(record.len(), 4);
                assert_eq!(record[0].offset, HexAddr(0xE));
                assert_eq!(record[0].equals, Some(HexAddr(0xB)));
                assert_eq!(record[1].min, Some(HexAddr(0x0106_0000)));
                assert_eq!(record[1].max, Some(HexAddr(0x0108_0000)));
                assert_eq!(record[2].eq_global.as_deref(), Some("timer_tens"));
                assert_eq!(
                    writes,
                    &vec![
                        TimerWrite { offset: HexAddr(0x6), value: 9 },
                        TimerWrite { offset: HexAddr(0xA), value: 9 },
                    ]
                );
            }
            other => panic!("expected the Located form, got {other:?}"),
        }

        // Reusable rejection helper: write the profile, expect a load error
        // whose message contains `needle`.
        let reject = |timer_hold: &str, needle: &str| {
            fs::write(game_dir.join("located.profile.json"), mk(timer_hold)).unwrap();
            let err = GameProfile::load(&game_dir).unwrap_err();
            assert!(err.contains(needle), "expected {needle:?} in: {err}");
        };

        // Bad predicate size (3 is not 1/2/4).
        reject(
            r#"{"scan":["0xC000","0xF000"],"record":[{"offset":"0xE","size":3,"equals":"0x0000000B"}],"writes":[{"offset":"0x6","value":9}]}"#,
            "predicate size 3",
        );
        // Two predicate kinds on one entry (equals AND min+max).
        reject(
            r#"{"scan":["0xC000","0xF000"],"record":[{"offset":"0xE","size":4,"equals":"0x0000000B","min":"0x0","max":"0x1"}],"writes":[{"offset":"0x6","value":9}]}"#,
            "exactly one of",
        );
        // eq_global naming an undeclared global.
        reject(
            r#"{"scan":["0xC000","0xF000"],"record":[{"offset":"0x6","size":1,"eq_global":"timer_typo"}],"writes":[{"offset":"0x6","value":9}]}"#,
            "names unknown global 'timer_typo'",
        );
        // Empty writes.
        reject(
            r#"{"scan":["0xC000","0xF000"],"record":[{"offset":"0xE","size":4,"equals":"0x0000000B"}],"writes":[]}"#,
            "has no writes",
        );
        // Empty / inverted scan window (start not < end).
        reject(
            r#"{"scan":["0xF000","0xC000"],"record":[{"offset":"0xE","size":4,"equals":"0x0000000B"}],"writes":[{"offset":"0x6","value":9}]}"#,
            "is not < end",
        );
        // A range predicate missing its upper bound.
        reject(
            r#"{"scan":["0xC000","0xF000"],"record":[{"offset":"0x2","size":4,"min":"0x0"}],"writes":[{"offset":"0x6","value":9}]}"#,
            "needs both min and max",
        );
    }

    #[test]
    fn hitstun_sources_valid() {
        // hitstun_sources with valid recorded globals
        let tmpbase = make_test_dir("hitstun_sources_valid");
        let game_dir = tmpbase.join("hitstun");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"hitstun","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"hitstun","port":"test","core":{"library_name":"","provenance_game":"hitstun","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{"combo_b1":"0x1000","combo_b2":"0x2000"},"record_globals":[{"name":"combo_b1","size":1},{"name":"combo_b2","size":1}]},"gate":[{"kind":"byte_zero","global":"combo_b1"}],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{},"hitstun_sources":{"block1":"combo_b1","block2":"combo_b2"}}"#;
        fs::write(game_dir.join("hitstun.profile.json"), port_json).unwrap();

        let result = GameProfile::load(&game_dir);
        assert!(result.is_ok());
    }

    #[test]
    fn hitstun_sources_unrecorded_global() {
        // hitstun_sources references a global not in the recorded union → error
        let tmpbase = make_test_dir("hitstun_sources_unrecorded");
        let game_dir = tmpbase.join("badhitstun");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"badhitstun","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"badhitstun","port":"test","core":{"library_name":"","provenance_game":"badhitstun","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{"combo_b1":"0x1000","unrecorded":"0x3000"}},"gate":[{"kind":"byte_zero","global":"combo_b1"}],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{},"hitstun_sources":{"block1":"unrecorded"}}"#;
        fs::write(game_dir.join("badhitstun.profile.json"), port_json).unwrap();

        let result = GameProfile::load(&game_dir);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("hitstun_sources names unrecorded global"));
    }

    #[test]
    fn mk2_ships_the_reptile_special_intersection() {
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        // Family order (moves list), only what the port encodes. §10 adds a
        // 4th reptile move (invisibility) — the original three's names AND
        // encodings below are asserted byte-identically, unmodified.
        let names: Vec<&str> = p.specials_for(9).iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["slide", "acid_spit", "force_ball", "invisibility"]);
        // slide: single chord step, back+LK+LP+Block (the §2 two-button
        // chord was live-DISPROVEN — the game performs a normal without
        // Block; slide pose + hit verified with Block in the chord).
        let (_, slide) = p.specials_for(9)[0];
        assert_eq!(slide.len(), 1);
        assert_eq!(slide[0].dirs, vec!["back"]);
        assert_eq!(slide[0].press, vec!["LK", "LP", "Block"]);
        assert_eq!(slide[0].frames, 8);
        // §10.1: invisibility ([BLK] U U D, release, HP) — Block held across
        // the U/U/D steps via while_held, released, then HP pressed.
        let (_, invis) = p.specials_for(9)[3];
        assert_eq!(invis.len(), 5);
        assert_eq!(invis[0].while_held, vec!["Block"]);
        assert_eq!(invis[3].release, vec!["Block"]);
        assert_eq!(invis[4].press, vec!["HP"]);
        // Characters without encodings offer nothing.
        assert!(p.specials_for(1).is_empty(), "liukang has no specials this phase");
        assert!(p.specials_for(99).is_empty());
        // Mileena (§10): sai_throw (hold+release), teleport_kick, roll.
        let mileena: Vec<&str> = p.specials_for(5).iter().map(|(n, _)| *n).collect();
        assert_eq!(mileena, vec!["sai_throw", "teleport_kick", "roll"]);
        let (_, sai_throw) = p.specials_for(5)[0];
        assert_eq!(sai_throw[0].hold, vec!["HP"]);
        // 34, not the transcribed 180: the charge threshold was bisected
        // live (33 fails 3/3, 34 fires 3/3) -- see mk2.md's live-audited
        // encodings section. The published "hold ~3 seconds" is ~5x the
        // real requirement.
        assert_eq!(sai_throw[0].min_frames, Some(34));
        assert_eq!(sai_throw[1].release, vec!["HP"]);
        // Membership, not a count. This assertion has now broken twice
        // because the lab did its job and encoded another character's kit;
        // a test that fails on success trains people to edit the number
        // until it passes. What must hold is that every declared special
        // survives compilation with a non-empty step list.
        assert!(p.all_specials().len() >= 7, "the shipped specials are present");
        for (name, steps) in p.all_specials() {
            assert!(!steps.is_empty(), "{name} compiled to no steps");
        }
        // The arcade contact signal is struct `health` (block+0x0E), the
        // frame lab's verified contact anchor: it steps by the whole damage
        // in ONE frame, on hit (161→150) AND on block (161→158 — blocked
        // normals chip 3/6/8 on this port, mk2.md). direction:"decrease"
        // makes it immune to the two INCREASE hazards (round-intro ramp,
        // training refill). hitstun_sources (the DRAWN HUD pair, 1 unit/
        // frame smear) stays as the fallback and the hitstun FEATURE source.
        // (An earlier `action_counter` contact claim was RETRACTED — mk2.md.)
        let cs = p.port.contact_signal.as_ref().expect("mk2 ships a contact_signal");
        assert_eq!(cs.field.as_deref(), Some("health"));
        assert!(cs.global.is_none());
        assert_eq!(cs.direction.as_deref(), Some("decrease"));
        let hs = p.port.hitstun_sources.as_ref().unwrap();
        assert_eq!(hs.get("block1").map(String::as_str), Some("p1_health_hud"));
        assert_eq!(hs.get("block2").map(String::as_str), Some("p2_health_hud"));
        // asurabld stays macro-free — that is the back-compat gate.
        let a = init_for_tests();
        assert!(a.family.moves.is_empty());
        assert!(a.all_specials().is_empty());
        assert!(a.port.contact_signal.is_none());
    }

    /// Task F2: the frame lab's `framelab` profile block (docs/frames.md
    /// §3.1/§4.1/§4.2, docs/game-profiles.md "The framelab block") is
    /// Python-only data — `PortProfile` has no field for it, deliberately,
    /// the same way it has none for `_STATUS`. This proves the Rust loader
    /// already TOLERATES an unrecognized top-level key rather than
    /// rejecting the profile: serde ignores unmapped JSON object members by
    /// default, and nothing in `profile.rs` opts into
    /// `#[serde(deny_unknown_fields)]` (grepping the file finds none). If a
    /// future change added that attribute to `PortProfile`, this test would
    /// fail the moment mk2's shipped `framelab` block tried to load, which
    /// is exactly the tripwire the task asked for.
    #[test]
    fn framelab_block_is_tolerated_as_an_unrecognized_top_level_key() {
        // The shipped file: loads clean, AND actually carries the block —
        // otherwise this test would be proving tolerance of a key that was
        // never really there.
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        let raw: serde_json::Value = serde_json::from_str(
            &fs::read_to_string("library/mk2/mk2.profile.json").unwrap(),
        )
        .unwrap();
        assert!(
            raw.get("framelab").is_some(),
            "mk2.profile.json should carry a framelab block for this test to mean anything"
        );
        // A few ordinary fields still resolve normally alongside it.
        assert_eq!(p.block1(), 0xC050);
        assert_eq!(p.field_off("health"), Some((0xE, 1)));

        // A synthetic profile with a NOVEL unknown key (not just this
        // task's own) also loads — the tolerance is general, not specific
        // to the string "framelab".
        let dir = make_test_dir("unknown_top_level_key").join("g");
        fs::create_dir(&dir).unwrap();
        fs::write(
            dir.join("family.json"),
            r#"{"family":"g","roster":[],"move_classes":[],"attack_classes":[]}"#,
        )
        .unwrap();
        fs::write(
            dir.join("g.profile.json"),
            r#"{"family":"g","port":"test",
                "core":{"library_name":"","provenance_game":"g","provenance_core":"g"},
                "memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},
                          "fighter_fields":[],"globals":{}},
                "gate":[],
                "enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],
                               "credits_target":0,"credits_min":0},
                "calibration":{},"attack_chords":{},
                "some_totally_unrelated_future_block":{"nested":[1,2,3]}}"#,
        )
        .unwrap();
        let result = GameProfile::load(&dir);
        assert!(
            result.is_ok(),
            "an unrecognized top-level key must not fail profile loading: {:?}",
            result.err()
        );
    }

    /// Build a family+port pair with macro fields spliced in, and load it.
    fn load_with_macros(tag: &str, moves: &str, port_extra: &str) -> Result<GameProfile, String> {
        let dir = make_test_dir(tag).join("g");
        fs::create_dir(&dir).unwrap();
        let family_json = format!(
            r#"{{"family":"g","roster":[{{"id":9,"name":"reptile"}}],"move_classes":[],
                "attack_classes":["None","LK","LP"],"moves":{moves}}}"#
        );
        fs::write(dir.join("family.json"), family_json).unwrap();
        let port_json = format!(
            r#"{{"family":"g","port":"test","core":{{"library_name":"","provenance_game":"g","provenance_core":"g"}},
                "memory":{{"blocks":{{"block1":"0x0","block2":"0x0","stride":"0x0"}},"fighter_fields":[],"globals":{{"hits":"0x100"}}}},
                "gate":[],"enforcement":{{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0}},
                "calibration":{{}},"attack_chords":{{"LK":["a"],"LP":["b"]}}{port_extra}}}"#
        );
        fs::write(dir.join("g.profile.json"), port_json).unwrap();
        GameProfile::load(&dir)
    }

    #[test]
    fn macro_schema_validation_rejects_bad_references() {
        let moves = r#"{"reptile":[{"name":"slide","tags":["special"]}]}"#;
        // Valid baseline parses.
        assert!(load_with_macros(
            "macros_ok", moves,
            r#","special_inputs":{"reptile":{"slide":[{"dirs":["back"],"press":["LK","LP"]}]}},"contact_signal":{"global":"hits"}"#
        ).is_ok());
        // moves keyed by a name not in the roster.
        assert!(load_with_macros("macros_badchar", r#"{"ghost":[{"name":"boo"}]}"#, "")
            .unwrap_err()
            .contains("unknown roster character"));
        // special_inputs for a character with no family moves.
        assert!(load_with_macros(
            "macros_nochar", "{}",
            r#","special_inputs":{"reptile":{"slide":[{"press":["LK"]}]}}"#
        ).unwrap_err().contains("absent from family moves"));
        // Encoding an undeclared move.
        assert!(load_with_macros(
            "macros_badmove", moves,
            r#","special_inputs":{"reptile":{"warp":[{"press":["LK"]}]}}"#
        ).unwrap_err().contains("unknown move 'warp'"));
        // Dir outside the closed semantic set.
        assert!(load_with_macros(
            "macros_baddir", moves,
            r#","special_inputs":{"reptile":{"slide":[{"dirs":["left"],"press":["LK"]}]}}"#
        ).unwrap_err().contains("unknown dir 'left'"));
        // Press class with no chord entry.
        assert!(load_with_macros(
            "macros_badclass", moves,
            r#","special_inputs":{"reptile":{"slide":[{"press":["HP"]}]}}"#
        ).unwrap_err().contains("unknown class 'HP'"));
        // Empty step / empty step list.
        assert!(load_with_macros(
            "macros_emptystep", moves,
            r#","special_inputs":{"reptile":{"slide":[{}]}}"#
        ).unwrap_err().contains("empty step"));
        assert!(load_with_macros(
            "macros_nosteps", moves,
            r#","special_inputs":{"reptile":{"slide":[]}}"#
        ).unwrap_err().contains("no steps"));
        // contact_signal must name a mapped global.
        assert!(load_with_macros("macros_badsignal", moves, r#","contact_signal":{"global":"nope"}"#)
            .unwrap_err()
            .contains("contact_signal names unknown global"));
    }

    #[test]
    fn id_map_invalid_roster_id() {
        // id_map references a non-existent roster id → error
        let tmpbase = make_test_dir("id_map_invalid_roster");
        let game_dir = tmpbase.join("badidmap");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"badidmap","roster":[{"id":0,"name":"a"},{"id":1,"name":"b"}],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"badidmap","port":"test","core":{"library_name":"","provenance_game":"badidmap","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{},"id_map":{"5":99}}"#;
        fs::write(game_dir.join("badidmap.profile.json"), port_json).unwrap();

        let result = GameProfile::load(&game_dir);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("id_map maps to unknown roster id"));
    }

    // ── object_ptr (docs/frames.md §5) ──────────────────────────────────

    #[test]
    fn object_ptr_decode_tms34010_bitaddr() {
        let ptr = ObjectPtr {
            off: SignedHex(-0xC),
            size: 4,
            encoding: "tms34010_bitaddr".to_string(),
            valid_range: [HexAddr(0x0100_0000), HexAddr(0x0140_0000)],
            cid_check_off: HexAddr(0x3E),
        };
        // (0x01010000 - 0x01000000) >> 3 = 0x10000 >> 3 = 0x2000.
        assert_eq!(ptr.decode(0x0101_0000), Some(0x2000));
        // Exactly at the low bound decodes to 0.
        assert_eq!(ptr.decode(0x0100_0000), Some(0));
        // Outside [lo, hi) — the pointer is not live this frame.
        assert_eq!(ptr.decode(0x00FF_FFFF), None);
        assert_eq!(ptr.decode(0x0140_0000), None); // hi is exclusive
        // Unknown encoding never guesses an address.
        let unknown = ObjectPtr { encoding: "future_thing".to_string(), ..ptr };
        assert_eq!(unknown.decode(0x0101_0000), None);
    }

    /// Build a family+port pair whose fighter fields exercise all three field
    /// forms at once (`off`, `via: "object_ptr"` unsigned, `via:
    /// "object_ptr"` signed) — the composition the task requires.
    fn load_with_object_ptr(tag: &str) -> GameProfile {
        let tmpbase = make_test_dir(tag);
        let dir = tmpbase.join("g");
        fs::create_dir(&dir).unwrap();
        let family_json = r#"{"family":"g","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(dir.join("family.json"), family_json).unwrap();
        let port_json = r#"{
            "family":"g","port":"test",
            "core":{"library_name":"","provenance_game":"g","provenance_core":"g"},
            "memory":{
                "cpu":"tms34010","endianness":"little",
                "blocks":{
                    "block1":"0x100","block2":"0x200","stride":"0x100",
                    "object_ptr":{
                        "off":"-0xC","size":4,"encoding":"tms34010_bitaddr",
                        "valid_range":["0x01000000","0x01400000"],"cid_check_off":"0x3E"
                    }
                },
                "fighter_fields":[
                    {"name":"char_id","off":"0x0","size":1},
                    {"name":"x","via":"object_ptr","off":"0x12","size":2},
                    {"name":"y","via":"object_ptr","off":"0x16","size":2,"signed":true}
                ],
                "globals":{}
            },
            "gate":[],
            "enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},
            "calibration":{},"attack_chords":{}
        }"#;
        fs::write(dir.join("g.profile.json"), port_json).unwrap();
        GameProfile::load(&dir).expect("object_ptr profile parses")
    }

    /// A byte-addressable little-endian synthetic RAM image, and the
    /// `read(addr, size) -> value` closure shape `object_ptr_field` expects.
    fn synth_reader(mem: BTreeMap<u32, u8>) -> impl FnMut(u32, u8) -> u32 {
        move |addr, size| {
            let mut v = 0u32;
            for i in 0..size as u32 {
                v |= (*mem.get(&(addr + i)).unwrap_or(&0) as u32) << (8 * i);
            }
            v
        }
    }

    #[test]
    fn object_ptr_field_composes_with_off_and_reads_x_and_signed_y() {
        let p = load_with_object_ptr("object_ptr_field_ok");
        // The plain `off` form still works unchanged.
        assert_eq!(p.field_off("char_id"), Some((0x0, 1)));
        // `via: "object_ptr"` fields have no fixed address.
        assert_eq!(p.field_off("x"), None);
        assert_eq!(p.field_addr(1, "x"), None);
        assert!(p.field_is_object_ptr("x"));
        assert!(p.field_is_object_ptr("y"));
        assert!(!p.field_is_object_ptr("char_id"));

        let mut mem = BTreeMap::new();
        // block1 = 0x100; pointer word at block1 - 0xC = 0xF4.
        // obj = (0x01010000 - 0x01000000) >> 3 = 0x2000.
        let raw_ptr: u32 = 0x0101_0000;
        for (i, b) in raw_ptr.to_le_bytes().iter().enumerate() {
            mem.insert(0xF4 + i as u32, *b);
        }
        mem.insert(0x100, 7); // block1's char_id
        mem.insert(0x2000 + 0x3E, 7); // obj's cross-check byte — matches
        // x = 300 at obj+0x12.
        for (i, b) in 300u16.to_le_bytes().iter().enumerate() {
            mem.insert(0x2000 + 0x12 + i as u32, *b);
        }
        // y = -5 at obj+0x16, signed.
        for (i, b) in (-5i16).to_le_bytes().iter().enumerate() {
            mem.insert(0x2000 + 0x16 + i as u32, *b);
        }

        assert_eq!(p.object_ptr_field(1, "x", synth_reader(mem.clone())), Some(300));
        assert_eq!(p.object_ptr_field(1, "y", synth_reader(mem.clone())), Some(-5));
        // Never a synthesized 0 — it round-trips the real negative value.
        assert_ne!(p.object_ptr_field(1, "y", synth_reader(mem.clone())), Some(0));
    }

    #[test]
    fn object_ptr_field_invalid_pointer_is_absent_not_zero() {
        let p = load_with_object_ptr("object_ptr_field_invalid_ptr");
        let mut mem = BTreeMap::new();
        // A pointer word OUTSIDE the valid range.
        let raw_ptr: u32 = 0x00FF_FFFF;
        for (i, b) in raw_ptr.to_le_bytes().iter().enumerate() {
            mem.insert(0xF4 + i as u32, *b);
        }
        mem.insert(0x100, 7);
        let v = p.object_ptr_field(1, "x", synth_reader(mem));
        assert_eq!(v, None, "an invalid pointer must yield ABSENT, never 0");
    }

    #[test]
    fn object_ptr_field_char_id_mismatch_is_absent_not_zero() {
        let p = load_with_object_ptr("object_ptr_field_stale");
        let mut mem = BTreeMap::new();
        let raw_ptr: u32 = 0x0101_0000; // decodes to obj = 0x2000, in range.
        for (i, b) in raw_ptr.to_le_bytes().iter().enumerate() {
            mem.insert(0xF4 + i as u32, *b);
        }
        mem.insert(0x100, 7); // block1's char_id
        mem.insert(0x2000 + 0x3E, 9); // obj's cross-check DISAGREES (stale slot)
        for (i, b) in 300u16.to_le_bytes().iter().enumerate() {
            mem.insert(0x2000 + 0x12 + i as u32, *b);
        }
        let v = p.object_ptr_field(1, "x", synth_reader(mem));
        assert_eq!(v, None, "a char_id mismatch must yield ABSENT, never a stale value");
    }

    #[test]
    fn object_ptr_via_rejects_bad_shapes() {
        // via names an unsupported encoding string at the field level.
        let tmpbase = make_test_dir("object_ptr_bad_via");
        let dir = tmpbase.join("g");
        fs::create_dir(&dir).unwrap();
        fs::write(
            dir.join("family.json"),
            r#"{"family":"g","roster":[],"move_classes":[],"attack_classes":[]}"#,
        )
        .unwrap();
        let base = |fighter_fields: &str, blocks_extra: &str| {
            format!(
                r#"{{"family":"g","port":"test",
                "core":{{"library_name":"","provenance_game":"g","provenance_core":"g"}},
                "memory":{{"blocks":{{"block1":"0x100","block2":"0x200","stride":"0x100"{blocks_extra}}},
                    "fighter_fields":[{fighter_fields}],"globals":{{}}}},
                "gate":[],
                "enforcement":{{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0}},
                "calibration":{{}},"attack_chords":{{}}}}"#
            )
        };
        // via="object_ptr" with no object_ptr declared on blocks.
        let json = base(r#"{"name":"x","via":"object_ptr","off":"0x12","size":2}"#, "");
        fs::write(dir.join("g.profile.json"), json).unwrap();
        let err = GameProfile::load(&dir).unwrap_err();
        assert!(err.contains("object_ptr is not declared"), "{err}");

        // via="object_ptr" with no off.
        let obj_ptr_json = r#","object_ptr":{"off":"-0xC","size":4,"encoding":"tms34010_bitaddr","valid_range":["0x01000000","0x01400000"],"cid_check_off":"0x3E"}"#;
        let json = base(r#"{"name":"x","via":"object_ptr","size":2}"#, obj_ptr_json);
        fs::write(dir.join("g.profile.json"), json).unwrap();
        let err = GameProfile::load(&dir).unwrap_err();
        assert!(err.contains("no off"), "{err}");

        // via="object_ptr" AND globals — pick one.
        let json = base(
            r#"{"name":"x","via":"object_ptr","off":"0x12","size":2,"globals":{"block1":"a","block2":"b"}}"#,
            obj_ptr_json,
        );
        fs::write(dir.join("g.profile.json"), json).unwrap();
        let err = GameProfile::load(&dir).unwrap_err();
        assert!(err.contains("pick one"), "{err}");

        // Unknown via name.
        let json = base(r#"{"name":"x","via":"vram_scan","off":"0x12","size":2}"#, obj_ptr_json);
        fs::write(dir.join("g.profile.json"), json).unwrap();
        let err = GameProfile::load(&dir).unwrap_err();
        assert!(err.contains("unknown via"), "{err}");

        // Valid via=object_ptr composes fine alongside a plain-off field.
        let json = base(
            r#"{"name":"char_id","off":"0x0","size":1},{"name":"x","via":"object_ptr","off":"0x12","size":2}"#,
            obj_ptr_json,
        );
        fs::write(dir.join("g.profile.json"), json).unwrap();
        assert!(GameProfile::load(&dir).is_ok());
    }

    #[test]
    fn signed_hex_parses_negative_and_positive() {
        let neg: SignedHex = serde_json::from_str("\"-0xC\"").unwrap();
        assert_eq!(neg.0, -12);
        let pos: SignedHex = serde_json::from_str("\"0x12\"").unwrap();
        assert_eq!(pos.0, 0x12);
        let num: SignedHex = serde_json::from_str("-12").unwrap();
        assert_eq!(num.0, -12);
    }

    /// mk2's shipped profile: `x` is pointer-resolved, `y` too (signed), and
    /// the DISPROVEN raw globals are gone (W2 cleanup): evidence lives in
    /// mk2.md, and a profile global is a machine-readable claim other tools
    /// may bind — history is not a reason to keep one.
    #[test]
    fn mk2_ships_x_and_y_as_object_ptr_fields() {
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        assert!(p.field_is_object_ptr("x"));
        assert!(p.field_is_object_ptr("y"));
        assert_eq!(p.field_addr(1, "x"), None, "no fixed address for a pointer-resolved field");
        assert_eq!(p.field_off("y"), None);
        // The disproven globals (object-pool-slot "positions", the constant
        // facing byte, the P1-victim-only hit_counter) are REMOVED, so no
        // consumer can quietly re-adopt them by name.
        for gone in ["p1_x", "p2_x", "p1_screen_x", "p1_facing", "hit_counter"] {
            assert!(p.global(gone).is_none(), "disproven global '{gone}' must stay removed");
        }
        assert!(p.port.memory.blocks.object_ptr.is_some());
        let obj_ptr = p.port.memory.blocks.object_ptr.as_ref().unwrap();
        assert_eq!(obj_ptr.off.0, -0xC);
        assert_eq!(obj_ptr.encoding, "tms34010_bitaddr");
    }

    // ── frame lab loader (docs/frames.md) ───────────────────────────────

    /// The shipped `library/mk2/arcade.frames.json` parses and collapses,
    /// every cell agreeing across its two observables — where "agrees" is
    /// now the CORRECTED rule (docs/frames.md §8.4): DIFFERENCE quantities
    /// (`on_hit`/`on_block`) and anchor-based absolutes
    /// (`first_active_frame`) must match to the frame, but ONE-SIDED
    /// quantities (`wakeup_window`) may legitimately differ by the two
    /// observables' `input_latency_frames` delta — that delta IS the
    /// observable's own manifestation margin, not noise. Before this fix the
    /// loader applied the exact-match rule to `wakeup_window` too and
    /// flagged Mileena's roll (77 vs 78, latencies 1 vs 2) as a
    /// disagreement it was not.
    ///
    /// Asserts the INVARIANT, not a snapshot of the row count. An earlier
    /// version pinned `cells.len() == 10` and `chars() == ["reptile"]`, and
    /// broke the moment a second character was measured — a test that fails
    /// because the lab did its job is a test that trains people to edit
    /// numbers until it passes. What must hold is: every cell carries one
    /// observation per observable and they agree.
    #[test]
    fn mk2_frames_json_parses_and_collapses_agreeing_observables() {
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        let table = p.frames.as_ref().expect("mk2 arcade ships a frames.json");
        assert_eq!(table.family, "mk2");
        assert_eq!(table.port, "arcade");
        assert!(!table.cells.is_empty(), "the shipped export has rows");
        for cell in &table.cells {
            assert!(
                cell.agrees(),
                "{} {:?} disagreed on {:?}",
                cell.move_name,
                cell.gap_walk_frames,
                cell.disagreements
            );
            assert_eq!(cell.observations.len(), 2);
        }

        // A specific close-range cell: HK at gap_walk_frames 60.
        let hk_close = table
            .cell("reptile", "HK", Some(60))
            .expect("HK close cell present");
        assert_eq!(hk_close.measurement.on_hit, Some(-7));
        assert_eq!(hk_close.measurement.on_block, Some(-14));
        assert_eq!(hk_close.measurement.first_active_frame, Some(11));
        assert_eq!(hk_close.variant.as_deref(), Some("close"));

        // Mileena's roll: the ONLY cross-observable raw difference anywhere
        // in the shipped file, and it's agreement, not disagreement.
        // struct_velocity read wakeup_window=77 at latency 1, pointer_x read
        // 78 at latency 2 -- 77-1 == 78-2 -- so it collapses instead of
        // flagging, and the collapsed 77 is in struct_velocity's frame of
        // reference (the smaller latency -> zero manifestation margin).
        let roll = table.cell("mileena", "roll", Some(0)).expect("mileena roll cell present");
        assert!(roll.agrees(), "the roll's wakeup_window is agreement, not disagreement: {:?}", roll.disagreements);
        assert_eq!(roll.measurement.wakeup_window, Some(77));
        assert_eq!(
            roll.one_sided_reference.get("wakeup_window").map(String::as_str),
            Some("struct_velocity"),
            "collapsed value is in the fastest (zero-margin) observable's frame of reference"
        );

        // The moves/gaps/chars accessors. Membership, not equality — the
        // roster of MEASURED characters grows as the lab runs.
        assert!(table.chars().contains(&"reptile"));
        assert!(table.chars().contains(&"mileena"));
        assert!(table.moves_for_char("reptile").contains(&"cHP"));
        for g in [30, 45, 60] {
            assert!(table.gaps_for_char("reptile").contains(&g), "gap {g} present");
        }
    }

    /// A knockdown move's `on_hit` is absent (NULL) because a knockdown has
    /// a wakeup clock, not a hit-advantage number (§1.1) — and absent must
    /// never collapse to 0.
    #[test]
    fn frames_null_on_hit_survives_as_absent_not_zero() {
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        let table = p.frames.as_ref().unwrap();
        let chp = table.cell("reptile", "cHP", Some(60)).expect("cHP cell present");
        assert_eq!(chp.measurement.on_hit, None, "knockdown gates on_hit to NULL, not 0");
        assert_ne!(chp.measurement.on_hit, Some(0));
        assert_eq!(chp.measurement.knockdown, Some(true));
        assert_eq!(chp.measurement.on_block, Some(-5), "on_block is still measured");
    }

    /// `safety_extremes` really returns the extremes of the shipped data.
    ///
    /// Asserts the RELATIONSHIP, not the numbers. This test previously
    /// pinned −16 and +13 and broke the moment a re-scan found a cell the
    /// old contact horizon had hidden — the third snapshot-style assertion
    /// in this file to fail because measurement progressed. A test that
    /// fails on success teaches people to edit the number until it passes,
    /// which is exactly how a wrong number gets blessed.
    #[test]
    fn frames_safety_extremes_match_shipped_data() {
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        let table = p.frames.as_ref().unwrap();
        let (unsafest, safest) = table.safety_extremes("reptile");
        let lo = unsafest.expect("an unsafest cell").measurement.on_block;
        let hi = safest.expect("a safest cell").measurement.on_block;
        assert!(lo.is_some() && hi.is_some(), "both extremes carry a number");
        assert!(lo <= hi, "unsafest {lo:?} must not exceed safest {hi:?}");

        // They must be the true extremes of every on_block this char has.
        let all: Vec<i64> = table
            .cells_for_char("reptile")
            .iter()
            .filter_map(|c| c.measurement.on_block)
            .collect();
        assert!(!all.is_empty(), "reptile has on_block rows");
        assert_eq!(lo, all.iter().copied().min(), "unsafest is the minimum");
        assert_eq!(hi, all.iter().copied().max(), "safest is the maximum");
    }

    /// A game with no `<port>.frames.json` loads cleanly and silently —
    /// asurabld ships none today.
    #[test]
    fn game_with_no_frames_file_loads_cleanly() {
        let p = GameProfile::load(Path::new("library/asurabld")).expect("asurabld profile loads");
        assert!(p.frames.is_none());
    }

    /// Builds a `CollapseInput` for the tests below — every one of them only
    /// needs an observable name, a latency, and a measurement.
    fn ci<'a>(
        observable: &'a str,
        input_latency_frames: Option<i64>,
        raw: &'a FrameMeasurement,
    ) -> CollapseInput<'a> {
        CollapseInput { observable, input_latency_frames, raw }
    }

    /// The collapse rule, tested directly: agreeing observations collapse
    /// field-by-field; a disagreeing field is left `None` and named, rather
    /// than one observable's value winning silently (§7, §12). `on_hit` is a
    /// DIFFERENCE quantity, so it still requires exact agreement even though
    /// the two observations here carry different latencies.
    #[test]
    fn collapse_measurements_flags_disagreement_without_picking_a_winner() {
        let mut a = FrameMeasurement::default();
        a.on_hit = Some(7);
        a.on_block = Some(-16);
        a.damage = Some(32);

        let mut b = a.clone();
        b.on_block = Some(-9); // the two observables disagree here

        let (collapsed, disagreements, one_sided) = collapse_measurements(&[
            ci("struct_velocity", Some(1), &a),
            ci("pointer_x", Some(2), &b),
        ]);
        assert_eq!(collapsed.on_hit, Some(7), "agreeing field still collapses");
        assert_eq!(collapsed.damage, Some(32));
        assert_eq!(collapsed.on_block, None, "disagreeing field is NOT silently resolved");
        assert_eq!(disagreements, vec!["on_block"]);
        assert!(one_sided.is_empty(), "on_block is a difference quantity, not one-sided");
    }

    /// The agreement path, tested directly (not just via the shipped file):
    /// identical observations collapse with zero disagreements.
    #[test]
    fn collapse_measurements_agreement_path_has_no_disagreements() {
        let mut a = FrameMeasurement::default();
        a.on_hit = Some(4);
        a.on_block = Some(13);
        a.knockdown = Some(false);
        let b = a.clone();

        let (collapsed, disagreements, one_sided) =
            collapse_measurements(&[ci("struct_velocity", Some(1), &a), ci("pointer_x", Some(2), &b)]);
        assert!(disagreements.is_empty());
        assert!(one_sided.is_empty(), "no one-sided field was measured here");
        assert_eq!(collapsed, a);
    }

    /// A ONE-SIDED quantity (`wakeup_window`) carries the measuring
    /// observable's own manifestation margin directly (docs/frames.md
    /// §4.2/§8.4): two sound observables legitimately differ by exactly
    /// their `input_latency_frames` delta. This isolates the Mileena-roll
    /// shape (77 vs 78, latencies 1 vs 2) from the shipped file.
    #[test]
    fn collapse_measurements_one_sided_field_agrees_within_latency_delta() {
        let mut a = FrameMeasurement::default();
        a.wakeup_window = Some(77);
        let mut b = FrameMeasurement::default();
        b.wakeup_window = Some(78);

        let (collapsed, disagreements, one_sided) = collapse_measurements(&[
            ci("struct_velocity", Some(1), &a),
            ci("pointer_x", Some(2), &b),
        ]);
        assert!(
            disagreements.is_empty(),
            "77 vs 78 at latencies 1 vs 2 is agreement, not disagreement: {disagreements:?}"
        );
        assert_eq!(collapsed.wakeup_window, Some(77));
        assert_eq!(
            one_sided.get("wakeup_window").map(String::as_str),
            Some("struct_velocity"),
            "collapsed value is in the smaller-latency (zero-margin) observable's frame"
        );
    }

    /// A one-sided field differing by anything OTHER than the latency delta
    /// is still a real disagreement — the corrected rule narrows what counts
    /// as agreement, it does not delete the check.
    #[test]
    fn collapse_measurements_one_sided_field_disagreeing_by_more_than_latency_is_flagged() {
        let mut a = FrameMeasurement::default();
        a.wakeup_window = Some(77);
        let mut b = FrameMeasurement::default();
        b.wakeup_window = Some(80); // off by 3, not the latency delta of 1

        let (collapsed, disagreements, one_sided) = collapse_measurements(&[
            ci("struct_velocity", Some(1), &a),
            ci("pointer_x", Some(2), &b),
        ]);
        assert_eq!(disagreements, vec!["wakeup_window"]);
        assert_eq!(collapsed.wakeup_window, None, "not silently resolved to either value");
        assert!(one_sided.is_empty());
    }

    /// A DIFFERENCE quantity does NOT get the one-sided leniency: its
    /// observable's margin already cancelled out of the subtraction (§4.3),
    /// so a raw difference — even one that happens to equal the observables'
    /// latency delta — is still a real disagreement.
    #[test]
    fn collapse_measurements_difference_quantity_ignores_latency_and_still_flags() {
        let mut a = FrameMeasurement::default();
        a.on_hit = Some(7);
        let mut b = FrameMeasurement::default();
        b.on_hit = Some(8); // == latency delta below, but on_hit isn't one-sided

        let (collapsed, disagreements, _) = collapse_measurements(&[
            ci("struct_velocity", Some(1), &a),
            ci("pointer_x", Some(2), &b),
        ]);
        assert_eq!(disagreements, vec!["on_hit"]);
        assert_eq!(collapsed.on_hit, None);
    }

    /// Missing latency data on a one-sided field falls back to the
    /// unconditional exact-match rule rather than guessing at an offset.
    #[test]
    fn collapse_measurements_one_sided_field_without_latency_falls_back_to_exact_match() {
        let mut a = FrameMeasurement::default();
        a.wakeup_window = Some(77);
        let mut b = FrameMeasurement::default();
        b.wakeup_window = Some(78);

        let (_, disagreements, one_sided) =
            collapse_measurements(&[ci("struct_velocity", None, &a), ci("pointer_x", Some(2), &b)]);
        assert_eq!(
            disagreements,
            vec!["wakeup_window"],
            "no latency to correct with on one side -- exact match applies"
        );
        assert!(one_sided.is_empty());
    }

    /// `total`/`recovery` are now ONE-SIDED, not anchor-based (docs/frames.md
    /// §8.4/§13 item 1, corrected): a whiff-anchored duration has no contact
    /// to bracket against, so it can only be measured the same way
    /// `wakeup_window` is — anchor to the act-again probe manifest — and
    /// therefore carries the same single margin. This is Reptile's
    /// invisibility: `total` reads 40 (`struct_velocity`, latency 1) and 41
    /// (`pointer_x`, latency 2), which is agreement (40−1 == 41−2), not the
    /// disagreement the old exact-match rule reported. Before this fix NO
    /// `total` could be stored for a whiff-anchored move at all.
    #[test]
    fn collapse_measurements_whiff_anchored_total_agrees_within_latency_delta() {
        let mut a = FrameMeasurement::default();
        a.total = Some(40);
        let mut b = FrameMeasurement::default();
        b.total = Some(41);

        let (collapsed, disagreements, one_sided) = collapse_measurements(&[
            ci("struct_velocity", Some(1), &a),
            ci("pointer_x", Some(2), &b),
        ]);
        assert!(
            disagreements.is_empty(),
            "40 vs 41 at latencies 1 vs 2 is agreement, not disagreement: {disagreements:?}"
        );
        assert_eq!(collapsed.total, Some(40));
        assert_eq!(
            one_sided.get("total").map(String::as_str),
            Some("struct_velocity"),
            "collapsed value is in the smaller-latency (zero-margin) observable's frame"
        );
    }

    /// The same field differing by anything OTHER than the latency delta is
    /// still a real disagreement — reclassifying `total` corrected what
    /// "agreement" means for it, it did not delete the check.
    #[test]
    fn collapse_measurements_whiff_anchored_total_disagreeing_by_more_than_latency_is_flagged() {
        let mut a = FrameMeasurement::default();
        a.total = Some(40);
        let mut b = FrameMeasurement::default();
        b.total = Some(45); // off by 5, not the latency delta of 1

        let (collapsed, disagreements, one_sided) = collapse_measurements(&[
            ci("struct_velocity", Some(1), &a),
            ci("pointer_x", Some(2), &b),
        ]);
        assert_eq!(disagreements, vec!["total"]);
        assert_eq!(collapsed.total, None, "not silently resolved to either value");
        assert!(one_sided.is_empty());
    }

    /// `recovery` gets the identical treatment as `total`, for the identical
    /// reason: both are read from a fixed anchor to the act-again probe
    /// manifest under the current protocol, so both carry exactly one
    /// observable margin.
    #[test]
    fn collapse_measurements_recovery_is_one_sided_like_total() {
        let mut a = FrameMeasurement::default();
        a.recovery = Some(20);
        let mut b = FrameMeasurement::default();
        b.recovery = Some(21);

        let (collapsed, disagreements, one_sided) = collapse_measurements(&[
            ci("struct_velocity", Some(1), &a),
            ci("pointer_x", Some(2), &b),
        ]);
        assert!(disagreements.is_empty());
        assert_eq!(collapsed.recovery, Some(20));
        assert_eq!(
            one_sided.get("recovery").map(String::as_str),
            Some("struct_velocity")
        );
    }

    /// `hitstop` is a DIFFERENCE quantity (connecting manifest minus
    /// whiffing manifest, §8.4) even though it is a duration — it must stay
    /// EXACT and must NOT get the one-sided leniency `total`/`recovery` just
    /// did, because unlike them it has agreed exactly on every cell measured
    /// so far across three characters, and reclassifying `total`/`recovery`
    /// must not disturb that.
    #[test]
    fn collapse_measurements_hitstop_disagreement_is_still_flagged_exactly() {
        let mut a = FrameMeasurement::default();
        a.hitstop = Some(10);
        let mut b = FrameMeasurement::default();
        b.hitstop = Some(11); // == latency delta below, but hitstop isn't one-sided

        let (collapsed, disagreements, one_sided) = collapse_measurements(&[
            ci("struct_velocity", Some(1), &a),
            ci("pointer_x", Some(2), &b),
        ]);
        assert_eq!(disagreements, vec!["hitstop"]);
        assert_eq!(collapsed.hitstop, None);
        assert!(one_sided.is_empty());
    }

    /// A malformed (but present) frames.json is a loud error, not a silent
    /// `None` — absence and corruption are different conditions.
    #[test]
    fn malformed_frames_json_is_a_loud_error() {
        let tmpbase = make_test_dir("badframes");
        let game_dir = tmpbase.join("mygame");
        fs::create_dir(&game_dir).unwrap();
        let family_json = r#"{"family":"mygame","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();
        let port_json = r#"{"family":"mygame","port":"arcade","core":{"library_name":"","provenance_game":"mygame","provenance_core":"mygame"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("mygame.profile.json"), port_json).unwrap();
        fs::write(game_dir.join("arcade.frames.json"), "not json").unwrap();

        let err = GameProfile::load(&game_dir).unwrap_err();
        assert!(err.contains("arcade.frames.json"), "{err}");
        let _ = fs::remove_dir_all(&tmpbase);
    }
}
