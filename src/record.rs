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

impl Default for GameMap {
    fn default() -> Self {
        GameMap {
            block1: 0x403798,
            block2: 0x40454C, // block1 + 0x0DB4
            round_timer: 0x40000A,
            round_over: 0x40646E,
            abort: 0x403678,
            match_end: 0x402A32,
            demo_flag: 0x4065D8,
            combo_on_b2: 0x4041E7,
            combo_on_b1: 0x40470B,
            credits: 0x40655D,
        }
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
    frames: u64,
    round_id: u64,
    prev_controllable: bool,
    p1_block: Option<u8>,
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
    pub fn create(path: &Path, map: GameMap, game: &str, core: &str) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let file = File::create(path)?;
        // Provenance sidecar (SPEC §5).
        let meta = serde_json::json!({
            "format": "jsonl-v2",
            "game": game,
            "core": core,
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
        Ok(FrameRecorder {
            map,
            out: BufWriter::new(file),
            frames: 0,
            round_id: 0,
            prev_controllable: false,
            p1_block: None,
        })
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
            && timer_bcd_valid(gate.timer_bcd);
        // A false->true edge starts a new round: bump the id and re-anchor
        // which block is P1 (left side starts with the smaller screen X).
        if controllable && !self.prev_controllable {
            self.round_id += 1;
            self.p1_block = Some(if b1.x <= b2.x { 1 } else { 2 });
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

    /// Flush the buffer (call at shutdown).
    pub fn finish(&mut self) {
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
        // A bare DebugState has no regions, so all reads return 0: healths are
        // 0 and the timer is 0, so the v2 gate must be CLOSED (v1's gate was
        // true here — the broken-permissive bug this rewrite fixes).
        let ds = DebugState::new();
        let path = std::env::temp_dir().join(format!("shadow_rec_{}.jsonl", std::process::id()));
        {
            let mut rec = FrameRecorder::create(&path, GameMap::default(), "test", "test").unwrap();
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
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("meta.json"));
    }
}
