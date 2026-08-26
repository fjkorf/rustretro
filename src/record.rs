//! Per-frame trace recorder for the behavioral-cloning "shadow" project.
//!
//! Writes one JSON object per emulated frame (JSONL), per `shadow/SPEC.md` §5
//! as amended by `shadow/PLAN.md` Wave 2a: RAW fields + the 12-bit RETRO input
//! masks + a composite `controllable` gate whose raw inputs are also recorded.
//! Normalization is deferred to train time, so this on-disk schema is
//! decoupled from the evolving feature schema. Streaming-appended so a long
//! session stays bounded in memory.
//!
//! v2 block model (see `library/asurabld/asurabld.md` §Fighter data blocks):
//! two 0x0DB4-stride fighter blocks whose slot→fighter assignment may vary,
//! so rows carry both blocks under neutral names plus a per-round `p1_block`
//! anchor resolved at round start (smaller X = left = P1). The demo-era
//! per-block hold accumulators track the OPPONENT's held direction and are
//! recorded under `opp_*` names to prevent misuse as self-features.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::debug::DebugState;

/// Fighter-block + system addresses for one game. Default = Asura Blade.
#[derive(Clone, Copy)]
pub struct GameMap {
    pub block1: u32,
    pub block2: u32,
    /// Round timer seconds, BCD (`$40000A`); subseconds at +1.
    pub round_timer: u32,
    /// Char-select countdown (`$400006`, BCD; 0 outside select). Gate v3:
    /// the v2 composite gate is TRUE on the char-select screen (healths +
    /// clock still read live there — probe-verified 2026-08-25).
    pub char_select: u32,
    /// Hop flags for the `controllable` gate (execution map, `asurabld.md`).
    pub round_over: u32,
    pub abort: u32,
    pub match_end: u32,
    /// Fight-loop discriminator candidate (`tst $4065D8` in the demo-fight
    /// loop) — recorded raw; semantics to be confirmed before gating on it.
    pub demo_flag: u32,
    /// Cross-block combo counters (nonzero = the OTHER fighter is in hitstun).
    pub combo_on_b2: u32, // $4041E7: block1's combo landing on block2
    pub combo_on_b1: u32, // $40470B: block2's combo landing on block1
    pub credits: u32,
}

impl GameMap {
    /// Populate every field from the loaded `GameProfile`'s blocks + named
    /// globals (see `docs/game-profiles.md`) — the data-driven replacement
    /// for the old compiled constants.
    pub fn from_profile(p: &crate::profile::GameProfile) -> GameMap {
        let g = |name: &str| {
            p.global(name)
                .unwrap_or_else(|| panic!("profile missing global '{name}'"))
        };
        GameMap {
            block1: p.block1(),
            block2: p.block2(),
            round_timer: g("round_timer"),
            char_select: g("char_select"),
            round_over: g("round_over"),
            abort: g("abort"),
            match_end: g("match_end"),
            demo_flag: g("demo_flag"),
            combo_on_b2: g("combo_on_b2"),
            combo_on_b1: g("combo_on_b1"),
            credits: g("credits"),
        }
    }
}

impl Default for GameMap {
    fn default() -> Self {
        GameMap::from_profile(crate::profile::current())
    }
}

/// One fighter block's raw fields (offsets per `asurabld.md`).
#[derive(serde::Serialize)]
struct Fighter {
    x: u16,        // +0x54 screen X
    y: u16,        // +0x56 screen Y (ground = 216)
    facing: u8,    // +0x61 (0 = facing left)
    weapon: u8,    // +0x65 (0 = armed)
    health: u8,    // +0x177 (max 0xEF; regenerates standing neutral)
    health2: u8,   // +0x179 paired health byte (2-stacked-bars hypothesis)
    meter: u8,     // +0x17B super meter
    meter_max: u8, // +0x17F per-character max meter constant
    char_id: u8,   // +0x639
    wins: u8,      // +0xA4C
    timer: u16,    // +0x00 free-running frame timer
    anim: u16,     // +0x12 walk/animation counter
    action: u16,   // +0x50 action/command index
    // CAVEAT: these accumulators track the OPPONENT's held direction (live-
    // verified 2026-08-24) — never use as self-features; kept for analysis.
    opp_right_hold: u16, // +0x28
    opp_left_hold: u16,  // +0x2A
}

/// Raw inputs to the `controllable` gate, recorded so training can re-derive
/// or tighten the gate without re-recording.
#[derive(serde::Serialize)]
struct Gate {
    round_over: u16,
    abort: u16,
    match_end: u16,
    timer_bcd: u8,
    /// Char-select countdown — must be 0 for `controllable` (gate v3);
    /// recorded raw like the other gate inputs (additive to jsonl-v2).
    char_sel: u8,
    demo_flag: u16,
    combo_on_b1: u8,
    combo_on_b2: u8,
    credits: u8,
}

#[derive(serde::Serialize)]
struct Row {
    frame: u64,
    round_id: u64,
    controllable: bool,
    /// Which block is P1 (left fighter at round start): 1, 2, or null before
    /// the first resolved round. Sticky within a round; sides may cross later.
    p1_block: Option<u8>,
    block1: Fighter,
    block2: Fighter,
    gate: Gate,
    p1_input: u16,
    p2_input: u16,
}

pub struct FrameRecorder {
    map: GameMap,
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
    round_chars: (u8, u8),
}

/// Read a guest 16-bit word big-endian (the 68k byte order) from a bus window
/// via the snapshot: `read_addr` returns little-endian, so swap.
fn u16be(ds: &DebugState, addr: u32) -> u16 {
    (ds.read_addr(addr as usize, 2).unwrap_or(0) as u16).swap_bytes()
}

fn u8g(ds: &DebugState, addr: u32) -> u8 {
    ds.read_addr(addr as usize, 1).unwrap_or(0) as u8
}

fn read_fighter(ds: &DebugState, base: u32) -> Fighter {
    Fighter {
        x: u16be(ds, base + 0x54),
        y: u16be(ds, base + 0x56),
        facing: u8g(ds, base + 0x61),
        weapon: u8g(ds, base + 0x65),
        health: u8g(ds, base + 0x177),
        health2: u8g(ds, base + 0x179),
        meter: u8g(ds, base + 0x17B),
        meter_max: u8g(ds, base + 0x17F),
        char_id: u8g(ds, base + 0x639),
        wins: u8g(ds, base + 0xA4C),
        timer: u16be(ds, base + 0x00),
        anim: u16be(ds, base + 0x12),
        action: u16be(ds, base + 0x50),
        opp_right_hold: u16be(ds, base + 0x28),
        opp_left_hold: u16be(ds, base + 0x2A),
    }
}

/// Both BCD nibbles decimal and the value nonzero (a live round clock —
/// includes frozen/held timers, excludes menu garbage like 0xFF).
fn timer_bcd_valid(t: u8) -> bool {
    t != 0 && (t >> 4) <= 9 && (t & 0xF) <= 9
}

/// Roster name for a char id (`+0x639`), read from the loaded profile's
/// `family.json` roster. Bosses/unknowns render as "c<N>".
pub fn char_name(id: u8) -> String {
    crate::profile::current().char_name(id)
}

/// Matchup slug matching `shadow_train.asurabld.matchup_slug(me, opp)` —
/// used to find per-matchup arenas (`shadow/arenas/<slug>.state`).
pub fn matchup_slug(me: u8, opp: u8) -> String {
    crate::profile::current().matchup_slug(me, opp)
}

/// The stage/opponent selector byte: freezing `$40364D` through the
/// post-select map screen forces the next fight's venue AND its home
/// character as the opponent (write-verified; see asurabld.md "Stages").
/// Selector value whose home character is `opp` — i.e. what to freeze the
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

impl FrameRecorder {
    /// Open `path` for a fresh recording (truncates) and drop a `.meta.json`
    /// sidecar next to it. `game`/`core` are free-form provenance strings.
    pub fn create(
        path: &Path,
        map: GameMap,
        game: &str,
        core: &str,
        style: Option<&str>,
    ) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let file = File::create(path)?;
        // Provenance sidecar (SPEC §5).
        let meta = serde_json::json!({
            "format": "jsonl-v2",
            "game": game,
            "core": core,
            "style": style,
            "fps": 60,
            "blocks": {
                "block1": format!("0x{:X}", map.block1),
                "block2": format!("0x{:X}", map.block2),
                "stride": format!("0x{:X}", map.block2.wrapping_sub(map.block1)),
            },
            "gate": "hop flags clear AND both healths in 1..=0xEF AND round timer valid BCD",
            "anchor": "p1_block resolved at round start: smaller X = left = P1",
            "note": "raw fields; opp_*_hold track the opponent's inputs; normalize at train time",
        });
        let meta_path: PathBuf = path.with_extension("meta.json");
        std::fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap_or_default()).ok();
        let rounds_out = File::create(path.with_extension("rounds.jsonl"))
            .ok()
            .map(BufWriter::new);
        Ok(FrameRecorder {
            map,
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
            round_chars: (0, 0),
        })
    }

    /// Append the finished (or aborted) round's summary to the rounds sidecar.
    /// `demo` mirrors the trainer's filter exactly: a round with zero total
    /// p1-input mass is attract-mode/CPU play, not a demonstration.
    fn emit_round_summary(&mut self) {
        let Some(out) = self.rounds_out.as_mut() else { return };
        let line = serde_json::json!({
            "round_id": self.round_id,
            "block1_char": self.round_chars.0,
            "block2_char": self.round_chars.1,
            "p1_block": self.p1_block,
            "frames": self.round_frames,
            "p1_input_mass": self.round_p1_mass,
            "demo": self.round_p1_mass == 0,
            "style": self.style,
        });
        let _ = out.write_all(line.to_string().as_bytes());
        let _ = out.write_all(b"\n");
        let _ = out.flush(); // rounds are rare; keep the index crash-current
    }

    /// Append one frame. `p1_mask`/`p2_mask` are the authoritative 12-bit RETRO
    /// input words for this frame (captured at the frontend input layer, since
    /// `$810000` is not in a bus window). Fighter fields are read from the live
    /// Work RAM snapshot.
    pub fn record(&mut self, ds: &DebugState, p1_mask: u16, p2_mask: u16) {
        let m = self.map;
        let b1 = read_fighter(ds, m.block1);
        let b2 = read_fighter(ds, m.block2);
        let gate = Gate {
            round_over: u16be(ds, m.round_over),
            abort: u16be(ds, m.abort),
            match_end: u16be(ds, m.match_end),
            timer_bcd: u8g(ds, m.round_timer),
            char_sel: u8g(ds, m.char_select),
            demo_flag: u16be(ds, m.demo_flag),
            combo_on_b1: u8g(ds, m.combo_on_b1),
            combo_on_b2: u8g(ds, m.combo_on_b2),
            credits: u8g(ds, m.credits),
        };
        // Gate v2 (PLAN Wave 2a): hop flags clear AND both fighters hold a
        // plausible live health AND the round clock reads as a BCD time.
        // Menus/continue keep stale health but latch a hop flag or corrupt the
        // clock; attract demo still passes — filter it at train time via the
        // recorded demo_flag/credits once their semantics are confirmed.
        let healthy = |f: &Fighter| (1..=0xEF).contains(&f.health);
        let controllable = gate.round_over == 0
            && gate.abort == 0
            && gate.match_end == 0
            && healthy(&b1)
            && healthy(&b2)
            && timer_bcd_valid(gate.timer_bcd)
            // Gate v3: the above is all TRUE on the char-select screen too
            // (probe-verified) — require its countdown to be over.
            && gate.char_sel == 0;
        // A false->true edge starts a new round: bump the id and re-anchor
        // which block is P1 (left side starts with the smaller screen X).
        if controllable && !self.prev_controllable {
            self.round_id += 1;
            self.p1_block = Some(if b1.x <= b2.x { 1 } else { 2 });
            self.round_frames = 0;
            self.round_p1_mass = 0;
            self.round_chars = (b1.char_id, b2.char_id);
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

        let row = Row {
            frame: self.frames,
            round_id: self.round_id,
            controllable,
            p1_block: self.p1_block,
            block1: b1,
            block2: b2,
            gate,
            p1_input: p1_mask,
            p2_input: p2_mask,
        };
        if let Ok(line) = serde_json::to_string(&row) {
            let _ = self.out.write_all(line.as_bytes());
            let _ = self.out.write_all(b"\n");
        }
        self.frames += 1;
        // Bound loss on a crash without flushing every frame.
        if self.frames % 60 == 0 {
            let _ = self.out.flush();
        }
    }

    pub fn frames_written(&self) -> u64 {
        self.frames
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

    #[test]
    fn pack_mask_packs_low_12_bits() {
        let mut b = [false; 12];
        b[0] = true;
        b[7] = true;
        b[11] = true;
        assert_eq!(pack_mask(&b), 0b1000_1000_0001);
    }

    #[test]
    fn timer_bcd_validity() {
        assert!(timer_bcd_valid(0x90));
        assert!(timer_bcd_valid(0x85));
        assert!(timer_bcd_valid(0x09));
        assert!(!timer_bcd_valid(0x00)); // expired / cleared
        assert!(!timer_bcd_valid(0xFF)); // menu garbage
        assert!(!timer_bcd_valid(0x3A)); // non-BCD low nibble
    }

    #[test]
    fn recorder_writes_valid_jsonl_and_gates_closed_without_state() {
        crate::profile::init_for_tests();
        // A bare DebugState has no regions, so all reads return 0: healths are
        // 0 and the timer is 0, so the v2 gate must be CLOSED (v1's gate was
        // true here — the broken-permissive bug this rewrite fixes).
        let ds = DebugState::new();
        let path = std::env::temp_dir().join(format!("shadow_rec_{}.jsonl", std::process::id()));
        {
            let mut rec =
                FrameRecorder::create(&path, GameMap::default(), "test", "test", None).unwrap();
            rec.record(&ds, 0x081, 0x000);
            rec.record(&ds, 0x000, 0x040);
            assert_eq!(rec.frames_written(), 2);
            rec.finish();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        for (i, l) in lines.iter().enumerate() {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            assert_eq!(v["frame"], i as u64);
            assert_eq!(v["controllable"], false);
            assert!(v["p1_block"].is_null());
            assert_eq!(v["round_id"], 0);
            assert!(v["block1"]["x"].is_u64());
            assert!(v["block1"]["health"].is_u64());
            assert!(v["gate"]["timer_bcd"].is_u64());
        }
        let v0: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v0["p1_input"], 0x081);
        // Gate never opened → the rounds index exists but holds no rounds.
        let rounds = std::fs::read_to_string(path.with_extension("rounds.jsonl")).unwrap();
        assert!(rounds.is_empty());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("meta.json"));
        let _ = std::fs::remove_file(path.with_extension("rounds.jsonl"));
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
    fn recorder_emits_round_summaries_to_the_rounds_sidecar() {
        crate::profile::init_for_tests();
        let mut ds = DebugState::new();
        assert!(ds.install_bus_window(crate::debug::BusWindowCfg {
            name: "wram-test".into(),
            addr: 0x400000,
            len: 0x7000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        let m = GameMap::default();
        // Open the v2 gate: live healths + valid BCD clock (hop flags are 0).
        assert!(ds.write_addr((m.block1 + 0x177) as usize, 1, 0xEF));
        assert!(ds.write_addr((m.block2 + 0x177) as usize, 1, 0xEF));
        assert!(ds.write_addr(m.round_timer as usize, 1, 0x90));
        // Matchup: Goat (1) vs Rose Mary (7).
        assert!(ds.write_addr((m.block1 + 0x639) as usize, 1, 1));
        assert!(ds.write_addr((m.block2 + 0x639) as usize, 1, 7));

        let path =
            std::env::temp_dir().join(format!("shadow_rounds_{}.jsonl", std::process::id()));
        {
            let mut rec =
                FrameRecorder::create(&path, m, "test", "test", Some("rushdown")).unwrap();
            rec.record(&ds, 0x010, 0); // rising edge; input held
            rec.record(&ds, 0x000, 0); // still live, idle
            // Corrupt the clock → gate closes → falling edge indexes the round.
            assert!(ds.write_addr(m.round_timer as usize, 1, 0xFF));
            rec.record(&ds, 0x000, 0);
            rec.finish();
        }
        let rounds = std::fs::read_to_string(path.with_extension("rounds.jsonl")).unwrap();
        let lines: Vec<&str> = rounds.lines().collect();
        assert_eq!(lines.len(), 1, "exactly one round summary: {rounds}");
        let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["round_id"], 1);
        assert_eq!(v["block1_char"], 1);
        assert_eq!(v["block2_char"], 7);
        assert_eq!(v["p1_block"], 1);
        assert_eq!(v["frames"], 2);
        assert_eq!(v["p1_input_mass"], 0x10);
        assert_eq!(v["demo"], false);
        assert_eq!(v["style"], "rushdown");
        // The meta sidecar carries the style declaration too.
        let meta = std::fs::read_to_string(path.with_extension("meta.json")).unwrap();
        assert!(meta.contains("\"style\": \"rushdown\""));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("meta.json"));
        let _ = std::fs::remove_file(path.with_extension("rounds.jsonl"));
    }
}
