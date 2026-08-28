//! Per-frame trace recorder for the behavioral-cloning "shadow" project.
//!
//! Writes one JSON object per emulated frame (jsonl-v3, the normative schema
//! in `shadow/RECORDER_V3.md` §1, amending `shadow/SPEC.md` §5): RAW values +
//! the 12-bit RETRO input masks + `controllable` from the profile's gate.
//! Normalization is deferred to train time, so this on-disk schema is
//! decoupled from the evolving feature schema. Streaming-appended so a long
//! session stays bounded in memory.
//!
//! v3 is profile-driven end to end — record.rs holds NO game addresses:
//! - `block1`/`block2` carry exactly the profile's `memory.fighter_fields`,
//!   by name, in profile order. An unmapped field is ABSENT from the row,
//!   never emitted as 0 (a partial map like library/mk2 records honestly
//!   sparse rows).
//! - `globals` carries every global the recorder samples, keyed by profile
//!   name: gate-referenced globals first (gate order), then the profile's
//!   `record_globals` (their order), duplicates once at first position.
//! - `controllable` is `gate::eval_gate` — the ONE gate shared with
//!   training enforcement and Lua `game.controllable()`; the recorder keeps
//!   no private composite.
//! - Rows serialize with a fixed key order (`v, frame, round_id,
//!   controllable, p1_block, block1, block2, globals, p1_input, p2_input`),
//!   hand-built so two recorders on identical state emit identical bytes.
//!
//! `p1_block` anchors which block is P1, resolved on each gate rising edge
//! and sticky for the round: smaller `x` = left = P1 when the profile maps
//! fighter field `x`; otherwise block1 is assumed P1 and the meta sidecar
//! records `"anchor": "fixed_slots"` so the honesty is on disk.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use sha2::Digest;

use crate::debug::DebugState;
use crate::profile::{GameProfile, GateCond};

/// One pre-resolved row slot: the profile name, its JSON key (pre-escaped,
/// quotes included), where to read (ABSOLUTE guest address — fighter-field
/// slots are resolved per block at create time, covering both the offset and
/// global-sourced variants), and how much. `size` 2 reads a u16 in the
/// profile's guest byte order, anything else a u8.
struct Slot {
    name: String,
    key: String,
    addr: u32,
    size: u8,
}

fn json_key(name: &str) -> String {
    serde_json::to_string(name).unwrap_or_else(|_| format!("\"{name}\""))
}

fn rd8(ds: &DebugState, addr: u32) -> u8 {
    ds.read_addr(addr as usize, 1).unwrap_or(0) as u8
}

/// Guest-order u16: `read_addr` returns little-endian, so swap for big-endian
/// guests (68k) — same convention as `gate::eval_gate`'s reads.
fn rd16(ds: &DebugState, addr: u32, little: bool) -> u16 {
    let v = ds.read_addr(addr as usize, 2).unwrap_or(0) as u16;
    if little { v } else { v.swap_bytes() }
}

fn read_sized(ds: &DebugState, addr: u32, size: u8, little: bool) -> u64 {
    match size {
        2 => rd16(ds, addr, little) as u64,
        _ => rd8(ds, addr) as u64,
    }
}

/// The recorded-globals union (RECORDER_V3 §1.2 rule 2): every gate-condition
/// global (gate order; `word_zero` reads u16, the byte conditions u8), then
/// every `record_globals` entry (profile order, own size). Duplicates appear
/// once, at first position. Load-time validation guarantees the names
/// resolve; an unmapped name is skipped with a warning rather than lied
/// about as address 0.
fn recorded_globals(p: &GameProfile) -> Vec<Slot> {
    let mut out: Vec<Slot> = Vec::new();
    let push = |name: &str, size: u8, out: &mut Vec<Slot>| {
        if out.iter().any(|s| s.name == name) {
            return;
        }
        match p.global(name) {
            Some(addr) => {
                out.push(Slot { name: name.to_string(), key: json_key(name), addr, size })
            }
            None => eprintln!("[record] warning: recorded global '{name}' is not mapped — skipped"),
        }
    };
    for cond in &p.port.gate {
        if let Some(name) = cond.global_name() {
            let size = if matches!(cond, GateCond::WordZero { .. } | GateCond::WordIn { .. }) { 2 } else { 1 };
            push(name, size, &mut out);
        }
    }
    for g in &p.port.memory.record_globals {
        push(&g.name, g.size, &mut out);
    }
    out
}

/// The port-profile file this profile came from, resolved for the meta
/// sidecar's provenance: the candidate in the family dir whose `"port"`
/// matches the loaded port, else the `<dirname>.profile.json` legacy default,
/// else a lone `*.profile.json`.
fn resolve_profile_file(p: &GameProfile) -> Option<PathBuf> {
    let dir = if p.dir.is_dir() { p.dir.clone() } else { p.dir.parent()?.to_path_buf() };
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|f| f.to_string_lossy().ends_with(".profile.json"))
        .collect();
    candidates.sort();
    let port_of = |f: &Path| -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(f).ok()?).ok()?;
        v.get("port")?.as_str().map(str::to_string)
    };
    let matches: Vec<&PathBuf> =
        candidates.iter().filter(|f| port_of(f).as_deref() == Some(&p.port.port)).collect();
    if matches.len() == 1 {
        return Some(matches[0].clone());
    }
    let default = dir
        .file_name()
        .map(|s| dir.join(format!("{}.profile.json", s.to_string_lossy())))
        .filter(|f| f.is_file());
    default.or_else(|| (candidates.len() == 1).then(|| candidates[0].clone()))
}

/// One gate condition serialized verbatim for the meta sidecar (GateCond has
/// no Serialize derive — the profile schema is deserialize-only by design).
fn gate_cond_json(cond: &GateCond) -> serde_json::Value {
    match cond {
        GateCond::ByteZero { global } => {
            serde_json::json!({"kind": "byte_zero", "global": global})
        }
        GateCond::WordZero { global } => {
            serde_json::json!({"kind": "word_zero", "global": global})
        }
        GateCond::WordIn { global, values } => {
            serde_json::json!({"kind": "word_in", "global": global, "values": values})
        }
        GateCond::HealthInRange { min, max } => {
            serde_json::json!({"kind": "health_in_range", "min": min, "max": max})
        }
        GateCond::BcdValidNonzero { global } => {
            serde_json::json!({"kind": "bcd_valid_nonzero", "global": global})
        }
    }
}

/// Roster name for a char id, read from the loaded profile's `family.json`
/// roster. Bosses/unknowns render as "c<N>".
pub fn char_name(id: u8) -> String {
    crate::profile::current().char_name(id)
}

/// Matchup slug matching `shadow_train.asurabld.matchup_slug(me, opp)` —
/// used to find per-matchup arenas (`shadow/arenas/<slug>.state`).
pub fn matchup_slug(me: u8, opp: u8) -> String {
    crate::profile::current().matchup_slug(me, opp)
}

/// The stage/opponent selector byte: freezing the profile's stage-select
/// global through the post-select map screen forces the next fight's venue
/// AND its home character as the opponent (write-verified; see asurabld.md
/// "Stages"). Selector value whose home character is `opp` — what to freeze the
/// profile's stage-select global to in order to fight `opp` next. Resolved
/// from the loaded profile's `stage_select.value_to_home_char` table.
pub fn stage_value_for_opponent(opp: u8) -> Option<u8> {
    crate::profile::current().stage_value_for_opponent(opp)
}

/// Inverse of [`stage_value_for_opponent`]: which character a frozen
/// selector value will summon.
pub fn opponent_for_stage_value(v: u8) -> Option<u8> {
    crate::profile::current().opponent_for_stage_value(v)
}

/// Pack a 12-button held state into the low 12 bits (RETRO_DEVICE_ID order).
pub fn pack_mask(bits: &[bool; 12]) -> u16 {
    let mut m = 0u16;
    for (i, b) in bits.iter().enumerate() {
        if *b {
            m |= 1 << i;
        }
    }
    m
}

pub struct FrameRecorder {
    /// The profile snapshot the recorder reads with (cloned at create so the
    /// gate, blocks, and field map stay pinned for the whole recording, and
    /// tests can record against a profile that is not the process one).
    profile: GameProfile,
    little: bool,
    /// Fighter fields in profile order, per block (`addr` = ABSOLUTE — block
    /// base + offset, or the per-block global for global-sourced fields).
    /// Same names/order in both; only addresses differ.
    fields1: Vec<Slot>,
    fields2: Vec<Slot>,
    /// The recorded-globals union in §1.2-rule-2 order (`addr` absolute).
    globals: Vec<Slot>,
    /// Index into `fields` of `x` (the round-start anchor) and `char_id`
    /// (round-summary matchups); None = unmapped for this port.
    x_idx: Option<usize>,
    char_idx: Option<usize>,
    /// Live special-move matchers over (p1, p2) input streams (MACRO_ACTIONS
    /// §3) — char-blind union of every encoding the profile carries, compiled
    /// once at create; None when the port encodes nothing (zero overhead).
    matchers: Option<(crate::macros::Matcher, crate::macros::Matcher)>,
    /// Per-round completion counts, union of both players (rounds sidecar).
    round_specials: std::collections::BTreeMap<String, u32>,
    // x-liveness sentinel (mk2.md: an object-pool slot can go stale between
    // boots): first controllable x sample, frames seen unchanged, and the
    // outcome latches (varied = proven alive, or warned once).
    x_base: Option<(u64, u64)>,
    x_ctrl: u64,
    x_alive: bool,
    x_warned: bool,
    out: BufWriter<File>,
    /// Where the jsonl is being written (for status display / stop messages).
    path: PathBuf,
    /// Per-round summary sidecar (`<name>.rounds.jsonl`) — the cheap coverage
    /// index the matchup tooling reads instead of parsing the full trace.
    rounds_out: Option<BufWriter<File>>,
    /// Play-style declaration for this whole recording ("rushdown", …),
    /// echoed into the meta sidecar and every round summary.
    style: Option<String>,
    frames: u64,
    round_id: u64,
    prev_controllable: bool,
    p1_block: Option<u8>,
    // Per-round accumulators for the summary line (reset on the rising edge).
    round_frames: u64,
    round_p1_mass: u64,
    /// RAW char ids latched at the rising edge; None when `char_id` is
    /// unmapped (the sidecar then reports null, never 0).
    round_chars: Option<(u8, u8)>,
}

impl FrameRecorder {
    /// Open `path` for a fresh recording (truncates) and drop the `.meta.json`
    /// provenance sidecar next to it (RECORDER_V3 §1.3). `game`/`core` are
    /// free-form provenance strings; everything schema-shaped comes from
    /// `profile`.
    pub fn create(
        path: &Path,
        profile: &GameProfile,
        game: &str,
        core: &str,
        style: Option<&str>,
    ) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let file = File::create(path)?;
        // ABSOLUTE per-block addresses, resolved once — covers both field
        // variants (block offset, or per-block globals like MK2 arcade's
        // world X). Profile load validated that every field resolves.
        let slots = |block: u8| -> Vec<Slot> {
            profile
                .port
                .memory
                .fighter_fields
                .iter()
                .map(|f| {
                    let (addr, size) = profile
                        .field_addr(block, &f.name)
                        .expect("profile load validates fighter fields");
                    Slot { name: f.name.clone(), key: json_key(&f.name), addr, size }
                })
                .collect()
        };
        let (fields1, fields2) = (slots(1), slots(2));
        let globals = recorded_globals(profile);
        let field_idx =
            |name: &str| profile.port.memory.fighter_fields.iter().position(|f| f.name == name);
        let x_idx = field_idx("x");
        let char_idx = field_idx("char_id");

        // Compile every special the profile encodes (family∩port, all
        // characters — the annotation is coverage data, and the train-side
        // matcher stays authoritative). An uncompilable encoding is skipped
        // with a warning, never fatal.
        let compiled: Vec<crate::macros::CompiledMacro> = profile
            .all_specials()
            .iter()
            .filter_map(|(name, steps)| match crate::macros::compile(name, steps, profile) {
                Ok(m) => Some(m),
                Err(e) => {
                    eprintln!("[record] warning: special skipped: {e}");
                    None
                }
            })
            .collect();
        let matchers = (!compiled.is_empty()).then(|| {
            (crate::macros::Matcher::new(compiled.clone()), crate::macros::Matcher::new(compiled))
        });

        // Provenance sidecar: everything the Python side needs to interpret
        // the file WITHOUT the recorder's profile — snapshot of the field
        // map, recorded-globals union, gate, and calibration, plus a hash of
        // the exact profile bytes (port profile ‖ family.json).
        let profile_file = resolve_profile_file(profile);
        let profile_sha256 = profile_file.as_ref().and_then(|pf| {
            let port_bytes = std::fs::read(pf).ok()?;
            let fam_dir = if profile.dir.is_dir() { &profile.dir } else { pf.parent()? };
            let fam_bytes = std::fs::read(fam_dir.join("family.json")).ok()?;
            let mut h = sha2::Sha256::new();
            h.update(&port_bytes);
            h.update(&fam_bytes);
            Some(format!("{:x}", h.finalize()))
        });
        let blocks = &profile.port.memory.blocks;
        let meta = serde_json::json!({
            "format": "jsonl-v3",
            "family": profile.family.family,
            "port": profile.port.port,
            "profile_file": profile_file
                .as_ref()
                .and_then(|f| f.file_name())
                .map(|f| f.to_string_lossy().into_owned()),
            "profile_sha256": profile_sha256,
            "game": game,
            "core": core,
            "style": style,
            "fps": 60,
            "anchor": if x_idx.is_some() { "smaller_x" } else { "fixed_slots" },
            "blocks": {
                "block1": format!("0x{:X}", blocks.block1.0),
                "block2": format!("0x{:X}", blocks.block2.0),
                "stride": format!("0x{:X}", blocks.stride.0),
            },
            "fighter_fields": profile.port.memory.fighter_fields.iter().map(|f| {
                match (&f.off, &f.globals) {
                    (Some(off), _) => serde_json::json!({
                        "name": f.name,
                        "off": format!("0x{:X}", off.0),
                        "size": f.size,
                    }),
                    // Global-sourced field (RECORDER_V3 §2.5): snapshot the
                    // per-block global names instead of an offset.
                    (None, Some(g)) => serde_json::json!({
                        "name": f.name,
                        "globals": {"block1": g.block1, "block2": g.block2},
                        "size": f.size,
                    }),
                    (None, None) => unreachable!("profile load validates fighter fields"),
                }
            }).collect::<Vec<_>>(),
            "globals_recorded": globals.iter().map(|s| {
                serde_json::json!({"name": s.name, "size": if s.size == 2 { 2 } else { 1 }})
            }).collect::<Vec<_>>(),
            "gate": profile.port.gate.iter().map(gate_cond_json).collect::<Vec<_>>(),
            "calibration": profile.port.calibration,
            "created": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        });
        let meta_path: PathBuf = path.with_extension("meta.json");
        std::fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap_or_default()).ok();
        let rounds_out = File::create(path.with_extension("rounds.jsonl"))
            .ok()
            .map(BufWriter::new);
        Ok(FrameRecorder {
            profile: profile.clone(),
            little: profile.port.memory.endianness == "little",
            fields1,
            fields2,
            globals,
            x_idx,
            char_idx,
            matchers,
            round_specials: std::collections::BTreeMap::new(),
            x_base: None,
            x_ctrl: 0,
            x_alive: false,
            x_warned: false,
            out: BufWriter::new(file),
            path: path.to_path_buf(),
            rounds_out,
            style: style.map(str::to_string),
            frames: 0,
            round_id: 0,
            prev_controllable: false,
            p1_block: None,
            round_frames: 0,
            round_p1_mass: 0,
            round_chars: None,
        })
    }

    /// Append the finished (or aborted) round's summary to the rounds sidecar
    /// (RECORDER_V3 §1.4). `demo` mirrors the trainer's filter exactly: a
    /// round with zero total p1-input mass is attract-mode/CPU play, not a
    /// demonstration. Char ids are CANONICAL (`GameProfile::canon_char_id`) —
    /// that is what keeps matchup slugs, coverage cells, and model-set keys
    /// port-blind; null (never 0) when the port maps no `char_id`.
    fn emit_round_summary(&mut self) {
        let Some(out) = self.rounds_out.as_mut() else { return };
        let (c1, c2) = match self.round_chars {
            Some((a, b)) => (
                serde_json::json!(self.profile.canon_char_id(a)),
                serde_json::json!(self.profile.canon_char_id(b)),
            ),
            None => (serde_json::Value::Null, serde_json::Value::Null),
        };
        let mut line = serde_json::json!({
            "round_id": self.round_id,
            "block1_char": c1,
            "block2_char": c2,
            "p1_block": self.p1_block,
            "frames": self.round_frames,
            "p1_input_mass": self.round_p1_mass,
            "demo": self.round_p1_mass == 0,
            "style": self.style,
            "family": self.profile.family.family,
            "port": self.profile.port.port,
            "v": 3,
        });
        // Per-round special counts (MACRO_ACTIONS §3), union of both players.
        // Omitted when empty so profiles without encodings emit unchanged
        // sidecar lines.
        if !self.round_specials.is_empty() {
            line["specials"] = serde_json::json!(self.round_specials);
        }
        let _ = out.write_all(line.to_string().as_bytes());
        let _ = out.write_all(b"\n");
        let _ = out.flush(); // rounds are rare; keep the index crash-current
    }

    /// Append one frame. `p1_mask`/`p2_mask` are the authoritative 12-bit RETRO
    /// input words for this frame (captured at the frontend input layer, since
    /// the input port is not in a bus window). Everything else is read from
    /// the live snapshot through the profile's map.
    pub fn record(&mut self, ds: &DebugState, p1_mask: u16, p2_mask: u16) {
        let read_block = |slots: &[Slot]| -> Vec<u64> {
            slots
                .iter()
                .map(|f| read_sized(ds, f.addr, f.size, self.little))
                .collect()
        };
        let b1 = read_block(&self.fields1);
        let b2 = read_block(&self.fields2);
        let gvals: Vec<u64> = self
            .globals
            .iter()
            .map(|g| read_sized(ds, g.addr, g.size, self.little))
            .collect();
        // The ONE gate (RECORDER_V3 §1.2 rule 3): identical to training
        // enforcement and Lua `game.controllable()`.
        let controllable = crate::gate::eval_gate(ds, &self.profile);
        // A false->true edge starts a new round: bump the id and re-anchor
        // which block is P1 — left side (smaller X) when the profile maps X,
        // else block1 by the fixed-slot assumption the meta declares.
        if controllable && !self.prev_controllable {
            self.round_id += 1;
            self.p1_block = Some(match self.x_idx {
                Some(i) if b1[i] > b2[i] => 2,
                _ => 1,
            });
            self.round_frames = 0;
            self.round_p1_mass = 0;
            self.round_chars = self.char_idx.map(|i| (b1[i] as u8, b2[i] as u8));
            self.round_specials.clear();
        }
        // A true->false edge ends one: index it (matchup, size, demo-ness).
        if !controllable && self.prev_controllable {
            self.emit_round_summary();
        }
        if controllable {
            self.round_frames += 1;
            self.round_p1_mass += p1_mask as u64;
        }
        self.prev_controllable = controllable;

        // x-liveness sentinel: 300 controllable frames of a byte-identical x
        // pair almost certainly means the object-pool slot moved between
        // boots (mk2.md caveat) — say so once, on stderr, per recording.
        if let Some(i) = self.x_idx {
            if controllable && !self.x_alive && !self.x_warned {
                let cur = (b1[i], b2[i]);
                match self.x_base {
                    Some(base) if base != cur => self.x_alive = true,
                    _ => {
                        self.x_base = Some(cur);
                        self.x_ctrl += 1;
                        if self.x_ctrl >= 300 {
                            self.x_warned = true;
                            eprintln!(
                                "[record] warning: x looks frozen — object slot may have moved (mk2.md)"
                            );
                        }
                    }
                }
            }
        }

        // Live special recognition (MACRO_ACTIONS §3): feed each port's mask
        // with its fighter's side (port→block via the round anchor; side =
        // sign(opp.x − me.x), left-fighter default when x is unmapped/tied).
        let (mut p1_special, mut p2_special) = (None::<String>, None::<String>);
        if let Some((m1, m2)) = self.matchers.as_mut() {
            let p1b: u8 = self.p1_block.unwrap_or(1);
            let xs = self.x_idx.map(|i| (b1[i], b2[i]));
            let opp_right = |me_block: u8| -> bool {
                match xs {
                    Some((x1, x2)) => {
                        let (xm, xo) = if me_block == 1 { (x1, x2) } else { (x2, x1) };
                        if xm == xo { me_block == 1 } else { xo > xm }
                    }
                    None => me_block == 1,
                }
            };
            let p2b: u8 = if p1b == 1 { 2 } else { 1 };
            p1_special = m1.feed(p1_mask, opp_right(p1b)).first().map(|n| n.to_string());
            p2_special = m2.feed(p2_mask, opp_right(p2b)).first().map(|n| n.to_string());
        }
        if controllable {
            for n in [&p1_special, &p2_special].into_iter().flatten() {
                *self.round_specials.entry(n.clone()).or_insert(0) += 1;
            }
        }

        // Hand-built row with the fixed v3 key order — deterministic bytes
        // (two recorders on identical state must emit identical lines).
        let mut line = String::with_capacity(96 + 24 * (2 * self.fields1.len() + self.globals.len()));
        let _ = write!(
            line,
            "{{\"v\":3,\"frame\":{},\"round_id\":{},\"controllable\":{},\"p1_block\":",
            self.frames, self.round_id, controllable
        );
        match self.p1_block {
            Some(b) => {
                let _ = write!(line, "{b}");
            }
            None => line.push_str("null"),
        }
        for (name, vals) in [("block1", &b1), ("block2", &b2)] {
            let _ = write!(line, ",\"{name}\":{{");
            for (i, (f, v)) in self.fields1.iter().zip(vals.iter()).enumerate() {
                let _ = write!(line, "{}{}:{v}", if i > 0 { "," } else { "" }, f.key);
            }
            line.push('}');
        }
        line.push_str(",\"globals\":{");
        for (i, (g, v)) in self.globals.iter().zip(gvals.iter()).enumerate() {
            let _ = write!(line, "{}{}:{v}", if i > 0 { "," } else { "" }, g.key);
        }
        let _ = write!(line, "}},\"p1_input\":{p1_mask},\"p2_input\":{p2_mask}");
        // §3 annotation: appended AFTER p2_input in the fixed key order;
        // absent when no special completed this frame.
        if let Some(n) = &p1_special {
            let _ = write!(line, ",\"p1_special\":{}", json_key(n));
        }
        if let Some(n) = &p2_special {
            let _ = write!(line, ",\"p2_special\":{}", json_key(n));
        }
        line.push('}');

        let _ = self.out.write_all(line.as_bytes());
        let _ = self.out.write_all(b"\n");
        self.frames += 1;
        // Bound loss on a crash without flushing every frame.
        if self.frames % 60 == 0 {
            let _ = self.out.flush();
        }
    }

    pub fn frames_written(&self) -> u64 {
        self.frames
    }

    #[cfg(test)]
    fn x_frozen_warning_fired(&self) -> bool {
        self.x_warned
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Flush the buffer (call at shutdown). A round still in progress gets a
    /// partial summary — stopping mid-round shouldn't lose its index entry.
    pub fn finish(&mut self) {
        if self.prev_controllable {
            self.emit_round_summary();
            self.prev_controllable = false;
        }
        let _ = self.out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{prefix}_{}.jsonl", std::process::id()))
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("meta.json"));
        let _ = std::fs::remove_file(path.with_extension("rounds.jsonl"));
    }

    #[test]
    fn pack_mask_packs_low_12_bits() {
        let mut b = [false; 12];
        b[0] = true;
        b[7] = true;
        b[11] = true;
        assert_eq!(pack_mask(&b), 0b1000_1000_0001);
    }

    #[test]
    fn stage_selector_mapping_inverts_cleanly() {
        crate::profile::init_for_tests();
        for opp in 0u8..=9 {
            match stage_value_for_opponent(opp) {
                Some(v) => assert_eq!(opponent_for_stage_value(v), Some(opp)),
                None => assert_eq!(opp, 3, "only footee lacks a selector value"),
            }
        }
        assert_eq!(opponent_for_stage_value(0), None); // 0 = unset
        assert_eq!(opponent_for_stage_value(10), None); // overflow
    }

    #[test]
    fn recorder_writes_v3_jsonl_and_gates_closed_without_state() {
        let p = crate::profile::init_for_tests();
        // A bare DebugState has no regions, so all reads return 0: healths are
        // 0, so `health_in_range` fails and the gate must be CLOSED (v1's gate
        // was true here — the broken-permissive bug the v2 rewrite fixed).
        let ds = DebugState::new();
        let path = tmp("shadow_rec_v3");
        {
            let mut rec = FrameRecorder::create(&path, p, "test", "test", None).unwrap();
            rec.record(&ds, 0x081, 0x000);
            rec.record(&ds, 0x000, 0x040);
            assert_eq!(rec.frames_written(), 2);
            rec.finish();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        for (i, l) in lines.iter().enumerate() {
            // The version marker is the FIRST key of every row (§1.1).
            assert!(l.starts_with("{\"v\":3,\"frame\":"), "row must lead with v:3: {l}");
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            assert_eq!(v["frame"], i as u64);
            assert_eq!(v["controllable"], false);
            assert!(v["p1_block"].is_null());
            assert_eq!(v["round_id"], 0);
            assert!(v["block1"]["x"].is_u64());
            assert!(v["block1"]["health"].is_u64());
            assert!(v["globals"]["round_timer"].is_u64());
        }
        let v0: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v0["p1_input"], 0x081);
        // Gate never opened → the rounds index exists but holds no rounds.
        let rounds = std::fs::read_to_string(path.with_extension("rounds.jsonl")).unwrap();
        assert!(rounds.is_empty());
        cleanup(&path);
    }

    /// G3: the serialized row for the asurabld profile — exact bytes (key
    /// names + order per §1.2 rule 6), gate parity with `gate::eval_gate` on an
    /// open AND a closed frame, and writer determinism (two recorders on
    /// identical state emit identical files).
    #[test]
    fn v3_row_text_matches_contract_order_and_eval_gate() {
        let p = crate::profile::init_for_tests();
        let mut ds = DebugState::new();
        assert!(ds.install_bus_window(crate::debug::BusWindowCfg {
            name: "wram-test".into(),
            addr: 0x400000,
            len: 0x7000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        let off = |name: &str| p.field_off(name).unwrap().0;
        // Open the gate: live healths + valid BCD clock (hop flags read 0).
        assert!(ds.write_addr((p.block1() + off("health")) as usize, 1, 0xEF));
        assert!(ds.write_addr((p.block2() + off("health")) as usize, 1, 0xEF));
        assert!(ds.write_addr(p.global("round_timer").unwrap() as usize, 1, 0x90));
        // Matchup Goat (1) vs Rose Mary (7); block1 left of block2.
        assert!(ds.write_addr((p.block1() + off("char_id")) as usize, 1, 1));
        assert!(ds.write_addr((p.block2() + off("char_id")) as usize, 1, 7));
        assert!(ds.write_addr((p.block1() + off("x") + 1) as usize, 1, 100));
        assert!(ds.write_addr((p.block2() + off("x") + 1) as usize, 1, 200));
        assert!(crate::gate::eval_gate(&ds, p), "gate must be open on this state");

        let path = tmp("shadow_rec_exact");
        let path_b = tmp("shadow_rec_exact_twin");
        {
            let mut rec = FrameRecorder::create(&path, p, "test", "test", None).unwrap();
            let mut twin = FrameRecorder::create(&path_b, p, "test", "test", None).unwrap();
            rec.record(&ds, 0x081, 0x000);
            twin.record(&ds, 0x081, 0x000);
            // Corrupt the clock → eval_gate closes → controllable follows.
            assert!(ds.write_addr(p.global("round_timer").unwrap() as usize, 1, 0xFF));
            assert!(!crate::gate::eval_gate(&ds, p));
            rec.record(&ds, 0x000, 0x000);
            twin.record(&ds, 0x000, 0x000);
            rec.finish();
            twin.finish();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        // The full serialized open-gate row, byte for byte (fields in profile
        // order incl. the §2.4 opp-hold pair; globals gate-order-then-
        // record_globals-order).
        let expected = concat!(
            "{\"v\":3,\"frame\":0,\"round_id\":1,\"controllable\":true,\"p1_block\":1,",
            "\"block1\":{\"timer\":0,\"anim\":0,\"action\":0,\"x\":100,\"y\":0,",
            "\"facing\":0,\"weapon\":0,\"health\":239,\"health2\":0,\"meter\":0,",
            "\"meter_max\":0,\"char_id\":1,\"wins\":0,\"opp_right_hold\":0,\"opp_left_hold\":0},",
            "\"block2\":{\"timer\":0,\"anim\":0,\"action\":0,\"x\":200,\"y\":0,",
            "\"facing\":0,\"weapon\":0,\"health\":239,\"health2\":0,\"meter\":0,",
            "\"meter_max\":0,\"char_id\":7,\"wins\":0,\"opp_right_hold\":0,\"opp_left_hold\":0},",
            "\"globals\":{\"round_over\":0,\"abort\":0,\"match_end\":0,\"round_timer\":144,",
            "\"char_select\":0,\"combo_on_b2\":0,\"combo_on_b1\":0,\"demo_flag\":0,\"credits\":0},",
            "\"p1_input\":129,\"p2_input\":0}"
        );
        assert_eq!(lines[0], expected);
        // Closed frame: controllable mirrors eval_gate; round context sticks.
        let v1: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(v1["controllable"], false);
        assert_eq!(v1["round_id"], 1);
        assert_eq!(v1["p1_block"], 1);
        // Determinism: the twin recorder saw identical state → identical bytes.
        assert_eq!(text, std::fs::read_to_string(&path_b).unwrap());
        // Meta sidecar (§1.3): schema snapshot + provenance hash.
        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path.with_extension("meta.json")).unwrap())
                .unwrap();
        assert_eq!(meta["format"], "jsonl-v3");
        assert_eq!(meta["anchor"], "smaller_x");
        assert_eq!(meta["family"], "asurabld");
        assert_eq!(meta["profile_file"], "asurabld.profile.json");
        assert_eq!(meta["profile_sha256"].as_str().unwrap().len(), 64);
        assert_eq!(meta["gate"].as_array().unwrap().len(), 6);
        let recorded: Vec<&str> = meta["globals_recorded"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            recorded,
            vec![
                "round_over", "abort", "match_end", "round_timer", "char_select",
                "combo_on_b2", "combo_on_b1", "demo_flag", "credits"
            ]
        );
        assert_eq!(meta["fighter_fields"].as_array().unwrap().len(), 15);
        cleanup(&path);
        cleanup(&path_b);
    }

    /// G3: a partial profile (library/mk2) records ONLY its mapped fields —
    /// absent means absent, never 0 — with the fixed-slot anchor declared in
    /// the meta and canonical char ids in the rounds sidecar.
    #[test]
    fn v3_partial_profile_records_only_mapped_fields() {
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        let mut ds = DebugState::new();
        assert!(ds.install_bus_window(crate::debug::BusWindowCfg {
            name: "wram-mk2-test".into(),
            addr: 0x0,
            len: 0x10000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        // Open mk2's gate: screen_state/round_over read 0; put both healths
        // in range and give both fighters a raw char id.
        let hoff = p.field_off("health").unwrap().0;
        let coff = p.field_off("char_id").unwrap().0;
        assert!(ds.write_addr((p.block1() + hoff) as usize, 1, 100));
        assert!(ds.write_addr((p.block2() + hoff) as usize, 1, 90));
        assert!(ds.write_addr((p.block1() + coff) as usize, 1, 7));
        assert!(ds.write_addr((p.block2() + coff) as usize, 1, 9));
        assert!(crate::gate::eval_gate(&ds, &p));

        let path = tmp("shadow_rec_mk2");
        {
            let mut rec = FrameRecorder::create(&path, &p, "mk2", "fbneo", None).unwrap();
            rec.record(&ds, 0x010, 0x000);
            rec.finish(); // mid-round stop → partial summary
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert!(text.starts_with("{\"v\":3,"));
        for block in ["block1", "block2"] {
            let keys: Vec<&str> = v[block].as_object().unwrap().keys().map(|k| k.as_str()).collect();
            // Exactly the profile's fighter_fields, in profile order — no
            // zero-filled asurabld fields. `x` is GLOBAL-SOURCED (p1_x/p2_x,
            // §2.5) yet appears as a normal named field.
            assert_eq!(keys, vec!["char_id", "health", "x"], "{block} carries only mapped fields");
        }
        assert!(v["block1"]["y"].is_null(), "unmapped field must be ABSENT");
        assert_eq!(v["block1"]["health"], 100);
        let gkeys: Vec<&str> = v["globals"].as_object().unwrap().keys().map(|k| k.as_str()).collect();
        // Set membership (Value maps sort): gate globals + record_globals.
        assert_eq!(
            gkeys,
            vec!["hit_counter", "p1_health_hud", "p2_health_hud", "round_over", "screen_state"]
        );
        // Serialized order is gate order (word-read screen_state, round_over)
        // then record_globals order (the hitstun-source HUD pair, hit_counter).
        assert!(text.contains(
            "\"globals\":{\"screen_state\":0,\"round_over\":0,\
             \"p1_health_hud\":0,\"p2_health_hud\":0,\"hit_counter\":0}"
        ));
        assert!(text.contains("\"block1\":{\"char_id\":7,\"health\":100,\"x\":0}"));
        assert_eq!(v["controllable"], true);
        assert_eq!(v["p1_block"], 1, "equal x (both unwritten) → block1 anchors as P1");
        // Meta declares the smaller-x anchor (x is mapped, via globals) +
        // mk2 provenance; the global-sourced field snapshots its names.
        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path.with_extension("meta.json")).unwrap())
                .unwrap();
        assert_eq!(meta["format"], "jsonl-v3");
        assert_eq!(meta["anchor"], "smaller_x");
        assert_eq!(meta["family"], "mk2");
        assert_eq!(meta["port"], "arcade");
        assert_eq!(meta["profile_file"], "mk2.profile.json");
        let ff = meta["fighter_fields"].as_array().unwrap();
        assert_eq!(ff.len(), 3);
        assert_eq!(ff[2]["name"], "x");
        assert!(ff[2]["off"].is_null(), "global-sourced field has no off");
        assert_eq!(ff[2]["globals"]["block1"], "p1_x");
        // Rounds sidecar: v3 marker, port, canonical ids (identity — no id_map).
        let rounds = std::fs::read_to_string(path.with_extension("rounds.jsonl")).unwrap();
        let r: serde_json::Value = serde_json::from_str(rounds.lines().next().unwrap()).unwrap();
        assert_eq!(r["v"], 3);
        assert_eq!(r["port"], "arcade");
        assert_eq!(r["block1_char"], 7);
        assert_eq!(r["block2_char"], 9);
        assert_eq!(r["p1_block"], 1);
        cleanup(&path);
    }

    /// MACRO_ACTIONS §3: the live matcher annotates completion frames with
    /// `p1_special`/`p2_special` AFTER `p2_input`, absent otherwise; the
    /// rounds sidecar counts the union; and the writer stays deterministic.
    #[test]
    fn v3_rows_annotate_specials_and_count_them_per_round() {
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        let mut ds = DebugState::new();
        assert!(ds.install_bus_window(crate::debug::BusWindowCfg {
            name: "wram-mk2-specials".into(),
            addr: 0x0,
            len: 0x10000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        let hoff = p.field_off("health").unwrap().0;
        let coff = p.field_off("char_id").unwrap().0;
        assert!(ds.write_addr((p.block1() + hoff) as usize, 1, 100));
        assert!(ds.write_addr((p.block2() + hoff) as usize, 1, 90));
        assert!(ds.write_addr((p.block1() + coff) as usize, 1, 9));
        assert!(ds.write_addr((p.block2() + coff) as usize, 1, 9));
        // block1 left of block2 → p1 = block1, facing right; p2 faces left.
        assert!(ds.write_addr(p.global("p1_x").unwrap() as usize, 2, 100));
        assert!(ds.write_addr(p.global("p2_x").unwrap() as usize, 2, 200));
        assert!(crate::gate::eval_gate(&ds, &p));

        let path = tmp("shadow_rec_specials");
        let path_b = tmp("shadow_rec_specials_twin");
        {
            let mut rec = FrameRecorder::create(&path, &p, "mk2", "fbneo", None).unwrap();
            let mut twin = FrameRecorder::create(&path_b, &p, "mk2", "fbneo", None).unwrap();
            let mut both = |ds: &DebugState, m1: u16, m2: u16, rec: &mut FrameRecorder, twin: &mut FrameRecorder| {
                rec.record(ds, m1, m2);
                twin.record(ds, m1, m2);
            };
            // P1 slide: back(Left,6)+LK(a,8)+LP(b,0)+Block(l,10) = 0x541.
            both(&ds, 0x541, 0, &mut rec, &mut twin);
            // Held chord: consumed, no re-annotation.
            both(&ds, 0x541, 0, &mut rec, &mut twin);
            // P2 slide mirrored: back is Right (7) on the right side = 0x581.
            both(&ds, 0, 0x581, &mut rec, &mut twin);
            // Close the gate → the round summary lands.
            assert!(ds.write_addr(p.global("screen_state").unwrap() as usize, 2, 262));
            both(&ds, 0, 0, &mut rec, &mut twin);
            rec.finish();
            twin.finish();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(
            lines[0].ends_with("\"p1_input\":1345,\"p2_input\":0,\"p1_special\":\"slide\"}"),
            "annotation appended after p2_input: {}",
            lines[0]
        );
        assert!(!lines[1].contains("special"), "held chord must not re-fire: {}", lines[1]);
        assert!(
            lines[2].ends_with("\"p1_input\":0,\"p2_input\":1409,\"p2_special\":\"slide\"}"),
            "mirrored p2 slide: {}",
            lines[2]
        );
        assert!(!lines[3].contains("special"));
        // Determinism holds with the matchers in the loop.
        assert_eq!(text, std::fs::read_to_string(&path_b).unwrap());
        // Rounds sidecar: the union count for the round.
        let rounds = std::fs::read_to_string(path.with_extension("rounds.jsonl")).unwrap();
        let r: serde_json::Value = serde_json::from_str(rounds.lines().next().unwrap()).unwrap();
        assert_eq!(r["specials"]["slide"], 2);
        cleanup(&path);
        cleanup(&path_b);
    }

    /// The x-liveness sentinel (mk2.md: object-pool slots go stale between
    /// boots): 300 controllable frames of byte-identical x warns once;
    /// any variance proves liveness and suppresses it.
    #[test]
    fn frozen_x_warns_after_300_controllable_frames() {
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        let mut ds = DebugState::new();
        assert!(ds.install_bus_window(crate::debug::BusWindowCfg {
            name: "wram-mk2-frozenx".into(),
            addr: 0x0,
            len: 0x10000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        let hoff = p.field_off("health").unwrap().0;
        assert!(ds.write_addr((p.block1() + hoff) as usize, 1, 100));
        assert!(ds.write_addr((p.block2() + hoff) as usize, 1, 90));
        assert!(crate::gate::eval_gate(&ds, &p));

        let path = tmp("shadow_rec_frozenx");
        let mut rec = FrameRecorder::create(&path, &p, "mk2", "fbneo", None).unwrap();
        for _ in 0..299 {
            rec.record(&ds, 0, 0);
        }
        assert!(!rec.x_frozen_warning_fired(), "not yet at the threshold");
        rec.record(&ds, 0, 0);
        assert!(rec.x_frozen_warning_fired(), "300th identical controllable frame warns");

        // Variance before the threshold suppresses it for good.
        let path2 = tmp("shadow_rec_livex");
        let mut rec2 = FrameRecorder::create(&path2, &p, "mk2", "fbneo", None).unwrap();
        rec2.record(&ds, 0, 0);
        assert!(ds.write_addr(p.global("p1_x").unwrap() as usize, 2, 7));
        for _ in 0..400 {
            rec2.record(&ds, 0, 0);
        }
        assert!(!rec2.x_frozen_warning_fired(), "x moved — no warning");
        rec.finish();
        rec2.finish();
        cleanup(&path);
        cleanup(&path2);
    }

    #[test]
    fn recorder_emits_round_summaries_to_the_rounds_sidecar() {
        let p = crate::profile::init_for_tests();
        let mut ds = DebugState::new();
        assert!(ds.install_bus_window(crate::debug::BusWindowCfg {
            name: "wram-rounds-test".into(),
            addr: 0x400000,
            len: 0x7000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        let off = |name: &str| p.field_off(name).unwrap().0;
        let timer = p.global("round_timer").unwrap() as usize;
        // Open the gate: live healths + valid BCD clock (hop flags are 0).
        assert!(ds.write_addr((p.block1() + off("health")) as usize, 1, 0xEF));
        assert!(ds.write_addr((p.block2() + off("health")) as usize, 1, 0xEF));
        assert!(ds.write_addr(timer, 1, 0x90));
        // Matchup: Goat (1) vs Rose Mary (7).
        assert!(ds.write_addr((p.block1() + off("char_id")) as usize, 1, 1));
        assert!(ds.write_addr((p.block2() + off("char_id")) as usize, 1, 7));

        let path = tmp("shadow_rounds");
        {
            let mut rec =
                FrameRecorder::create(&path, p, "test", "test", Some("rushdown")).unwrap();
            rec.record(&ds, 0x010, 0); // rising edge; input held
            rec.record(&ds, 0x000, 0); // still live, idle
            // Corrupt the clock → gate closes → falling edge indexes the round.
            assert!(ds.write_addr(timer, 1, 0xFF));
            rec.record(&ds, 0x000, 0);
            rec.finish();
        }
        let rounds = std::fs::read_to_string(path.with_extension("rounds.jsonl")).unwrap();
        let lines: Vec<&str> = rounds.lines().collect();
        assert_eq!(lines.len(), 1, "exactly one round summary: {rounds}");
        let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["round_id"], 1);
        assert_eq!(v["block1_char"], 1); // canonical == raw (no id_map)
        assert_eq!(v["block2_char"], 7);
        assert_eq!(v["p1_block"], 1);
        assert_eq!(v["frames"], 2);
        assert_eq!(v["p1_input_mass"], 0x10);
        assert_eq!(v["demo"], false);
        assert_eq!(v["style"], "rushdown");
        assert_eq!(v["family"], "asurabld");
        assert_eq!(v["port"], "arcade");
        assert_eq!(v["v"], 3);
        // The meta sidecar carries the style declaration too.
        let meta = std::fs::read_to_string(path.with_extension("meta.json")).unwrap();
        assert!(meta.contains("\"style\": \"rushdown\""));
        cleanup(&path);
    }
}
