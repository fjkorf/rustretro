//! Per-frame trace recorder for the behavioral-cloning "shadow" project.
//!
//! Writes one JSON object per emulated frame (JSONL), per `shadow/SPEC.md` §5:
//! RAW decoded actor fields + the 12-bit RETRO input masks + a `controllable`
//! gate. Normalization is deferred to train time, so this on-disk schema is
//! decoupled from the evolving feature schema. Streaming-appended so a long
//! session stays bounded in memory.
//!
//! The field layout below is Asura Blade (Fuuki FG-3); bases/offsets come from
//! `library/asurabld/asurabld.md` and all live inside the Work RAM bus window
//! (`$400000 + 0x10000`). Other games would supply a different [`ActorMap`].

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::debug::DebugState;

/// Actor-struct + hop-flag addresses for one game. Default = Asura Blade.
#[derive(Clone, Copy)]
pub struct ActorMap {
    pub p1_base: u32,
    pub p2_base: u32,
    /// Hop flags for the `controllable` gate (execution map, `asurabld.md`).
    pub round_over: u32,
    pub abort: u32,
    pub match_end: u32,
}

impl Default for ActorMap {
    fn default() -> Self {
        ActorMap {
            p1_base: 0x40454C,
            p2_base: 0x405300, // p1 + 0x0DB4
            round_over: 0x40646E,
            abort: 0x403678,
            match_end: 0x402A32,
        }
    }
}

#[derive(serde::Serialize)]
struct Actor {
    x: u16,
    y: u16,
    action: u16,
    action2: u16,
    timer: u16,
    anim: u16,
    anim2: u16,
    right_hold: u16,
    left_hold: u16,
    x2: u16,
    y2: u16,
    // Pending RAM fields (SPEC §1b) — emitted as null until mapped.
    health: Option<u16>,
    meter: Option<u16>,
    facing: Option<u16>,
}

#[derive(serde::Serialize)]
struct Row {
    frame: u64,
    round_id: u64,
    controllable: bool,
    p1: Actor,
    p2: Actor,
    p1_input: u16,
    p2_input: u16,
}

pub struct FrameRecorder {
    map: ActorMap,
    out: BufWriter<File>,
    frames: u64,
    round_id: u64,
    prev_controllable: bool,
}

/// Read a guest 16-bit word big-endian (the 68k byte order) from a bus window
/// via the snapshot: `read_addr` returns little-endian, so swap.
fn u16be(ds: &DebugState, addr: u32) -> u16 {
    (ds.read_addr(addr as usize, 2).unwrap_or(0) as u16).swap_bytes()
}

fn read_actor(ds: &DebugState, base: u32) -> Actor {
    Actor {
        x: u16be(ds, base + 0x54),
        y: u16be(ds, base + 0x56),
        action: u16be(ds, base + 0x50),
        action2: u16be(ds, base + 0x4C),
        timer: u16be(ds, base + 0x00),
        anim: u16be(ds, base + 0x12),
        anim2: u16be(ds, base + 0x14),
        right_hold: u16be(ds, base + 0x28),
        left_hold: u16be(ds, base + 0x2A),
        x2: u16be(ds, base + 0x5A),
        y2: u16be(ds, base + 0x5C),
        health: None,
        meter: None,
        facing: None,
    }
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
    pub fn create(path: &Path, map: ActorMap, game: &str, core: &str) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let file = File::create(path)?;
        // Provenance sidecar (SPEC §5).
        let meta = serde_json::json!({
            "format": "jsonl-v1",
            "game": game,
            "core": core,
            "fps": 60,
            "actor": {
                "p1_base": format!("0x{:X}", map.p1_base),
                "p2_base": format!("0x{:X}", map.p2_base),
                "stride": format!("0x{:X}", map.p2_base.wrapping_sub(map.p1_base)),
            },
            "note": "raw decoded fields; normalize at train time (shadow/SPEC.md)",
        });
        let meta_path: PathBuf = path.with_extension("meta.json");
        std::fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap_or_default()).ok();
        Ok(FrameRecorder {
            map,
            out: BufWriter::new(file),
            frames: 0,
            round_id: 0,
            prev_controllable: false,
        })
    }

    /// Append one frame. `p1_mask`/`p2_mask` are the authoritative 12-bit RETRO
    /// input words for this frame (captured at the frontend input layer, since
    /// `$810000` is not in a bus window). Actor fields are read from the live
    /// Work RAM snapshot.
    pub fn record(&mut self, ds: &DebugState, p1_mask: u16, p2_mask: u16) {
        let m = self.map;
        // controllable gate (SPEC §5): a live fighting frame — no round-over
        // latch, no abort, no match-end.
        let controllable = u16be(ds, m.round_over) == 0
            && u16be(ds, m.abort) == 0
            && u16be(ds, m.match_end) == 0;
        // A false->true edge starts a new round.
        if controllable && !self.prev_controllable {
            self.round_id += 1;
        }
        self.prev_controllable = controllable;

        let row = Row {
            frame: self.frames,
            round_id: self.round_id,
            controllable,
            p1: read_actor(ds, m.p1_base),
            p2: read_actor(ds, m.p2_base),
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
    fn recorder_writes_valid_jsonl() {
        // A bare DebugState has no regions, so actor reads return 0 and the
        // controllable gate (all hop flags 0) is true — enough to exercise the
        // writer + schema without a live core.
        let ds = DebugState::new();
        let path = std::env::temp_dir().join(format!("shadow_rec_{}.jsonl", std::process::id()));
        {
            let mut rec =
                FrameRecorder::create(&path, ActorMap::default(), "test", "test").unwrap();
            rec.record(&ds, 0x081, 0x000);
            rec.record(&ds, 0x000, 0x040);
            rec.record(&ds, 0x000, 0x000);
            assert_eq!(rec.frames_written(), 3);
            rec.finish();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        for (i, l) in lines.iter().enumerate() {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            assert_eq!(v["frame"], i as u64);
            assert_eq!(v["controllable"], true);
            assert!(v["p1"]["health"].is_null());
            assert!(v["p1"]["x"].is_u64());
        }
        let v0: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v0["p1_input"], 0x081);
        assert_eq!(v0["round_id"], 1); // false->true edge on first frame
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("meta.json"));
    }
}
