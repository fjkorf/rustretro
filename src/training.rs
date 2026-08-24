//! Training mode v1 (shadow PLAN Wave 2b): an infinite, resettable practice
//! fight for demonstration recording.
//!
//! Enabled with `--training`. Each emulated frame, [`tick`] enforces:
//! - **credits topped up** (`$40655D`) so Start always works,
//! - **round timer held** at 85 seconds (`$40000A/B`, BCD) — no timeouts,
//! - **health refill**: when either fighter drops below the threshold both
//!   health bytes are rewritten to max, so damage/hitstun stay visible but
//!   nobody is ever KO'd (toggle with F3),
//! - **dummy control**: a preset drives controller port 1 (F1 cycles
//!   Free / Stand / Crouch / Jump / Block) — Block holds away from the
//!   other fighter using the live block X positions,
//! - **position reset** (F2) and **finish round now** (F4) one-shots.
//!
//! All writes go through `DebugState::write_addr`, which routes bus-window
//! addresses onto the live 68k bus via the Sek write queue. Addresses are the
//! Asura Blade map (`library/asurabld/asurabld.md`); the gate mirrors the
//! recorder's composite (`src/record.rs`).

use crate::debug::{DebugState, DummyMode};

const BLOCK1: u32 = 0x403798;
const BLOCK2: u32 = 0x40454C;
const HEALTH_OFF: u32 = 0x177; // byte pair at +0x177/+0x179, max 0xEF
const X_OFF: u32 = 0x54;
const Y_OFF: u32 = 0x56;
const ROUND_TIMER: u32 = 0x40000A; // BCD seconds; subseconds at +1
const ROUND_STATE: u32 = 0x400000; // write 0 = finish round now
const CREDITS: u32 = 0x40655D;
const ROUND_OVER: u32 = 0x40646E;
const ABORT: u32 = 0x403678;
const MATCH_END: u32 = 0x402A32;

const HEALTH_MAX: u8 = 0xEF;
const REFILL_BELOW: u8 = 0x40;
/// Round-start X positions (left / right) used by position reset.
const RESET_X: (u16, u16) = (84, 232);
const GROUND_Y: u16 = 216;

fn rd8(ds: &DebugState, addr: u32) -> u8 {
    ds.read_addr(addr as usize, 1).unwrap_or(0) as u8
}

fn rd16be(ds: &DebugState, addr: u32) -> u16 {
    (ds.read_addr(addr as usize, 2).unwrap_or(0) as u16).swap_bytes()
}

fn wr8(ds: &mut DebugState, addr: u32, v: u8) {
    let _ = ds.write_addr(addr as usize, 1, v as u32);
}

fn wr16be(ds: &mut DebugState, addr: u32, v: u16) {
    // write_addr takes little-endian value bytes to ascending addresses; the
    // 68k stores big-endian, so swap so the guest reads `v`.
    let _ = ds.write_addr(addr as usize, 2, v.swap_bytes() as u32);
}

/// Same composite in-fight gate as the recorder (record.rs).
fn in_fight(ds: &DebugState) -> bool {
    let t = rd8(ds, ROUND_TIMER);
    let healthy = |b: u32| (1..=HEALTH_MAX).contains(&rd8(ds, b + HEALTH_OFF));
    rd16be(ds, ROUND_OVER) == 0
        && rd16be(ds, ABORT) == 0
        && rd16be(ds, MATCH_END) == 0
        && healthy(BLOCK1)
        && healthy(BLOCK2)
        && t != 0
        && (t >> 4) <= 9
        && (t & 0xF) <= 9
}

/// Run one training-mode frame. Called from `Frontend::run_frame` after the
/// bus-window refresh (reads see this frame's snapshot; writes drain to the
/// live bus next frame).
pub fn tick(ds: &mut DebugState, frame: u64) {
    if !ds.training.enabled {
        return;
    }
    // Credits top-up, checked once a second: Start must always work.
    if frame % 60 == 0 && rd8(ds, CREDITS) < 5 {
        wr8(ds, CREDITS, 9);
    }
    if !in_fight(ds) {
        return;
    }
    // Hold the round clock.
    wr8(ds, ROUND_TIMER, 0x85);
    wr8(ds, ROUND_TIMER + 1, 0x03);
    // Health refill: let damage show, never let anyone die.
    if ds.training.refill {
        for base in [BLOCK1, BLOCK2] {
            if rd8(ds, base + HEALTH_OFF) < REFILL_BELOW {
                wr8(ds, base + HEALTH_OFF, HEALTH_MAX);
                wr8(ds, base + HEALTH_OFF + 2, HEALTH_MAX);
            }
        }
    }
    // One-shots.
    if ds.training.reset_positions {
        ds.training.reset_positions = false;
        let (b1x, b2x) = if rd16be(ds, BLOCK1 + X_OFF) <= rd16be(ds, BLOCK2 + X_OFF) {
            RESET_X
        } else {
            (RESET_X.1, RESET_X.0)
        };
        wr16be(ds, BLOCK1 + X_OFF, b1x);
        wr16be(ds, BLOCK1 + Y_OFF, GROUND_Y);
        wr16be(ds, BLOCK2 + X_OFF, b2x);
        wr16be(ds, BLOCK2 + Y_OFF, GROUND_Y);
    }
    if ds.training.finish_round {
        ds.training.finish_round = false;
        wr8(ds, ROUND_STATE, 0);
    }
    // Dummy preset → port-1 injection (2-frame holds so they bridge to the
    // next GUI fold without latching).
    let dummy_bits: Option<[bool; 12]> = match ds.training.dummy {
        DummyMode::Free => None,
        DummyMode::Stand => Some([false; 12]),
        DummyMode::Crouch => {
            let mut b = [false; 12];
            b[5] = true; // Down
            Some(b)
        }
        DummyMode::Jump => {
            let mut b = [false; 12];
            // Tap Up half a second out of every second → repeated hops.
            b[4] = (frame / 30) % 2 == 0;
            Some(b)
        }
        DummyMode::Block => {
            // Hold away from the other fighter. The dummy is port 1; without a
            // resolved port→block map, treat the RIGHT fighter as the dummy
            // (P2 side) — correct for freshly started VS rounds.
            let x1 = rd16be(ds, BLOCK1 + X_OFF) as i32;
            let x2 = rd16be(ds, BLOCK2 + X_OFF) as i32;
            let (dummy_x, other_x) = if x1 >= x2 { (x1, x2) } else { (x2, x1) };
            let mut b = [false; 12];
            if dummy_x >= other_x {
                b[7] = true; // opponent to the left → hold Right (away)
            } else {
                b[6] = true;
            }
            Some(b)
        }
    };
    if let Some(bits) = dummy_bits {
        for (i, on) in bits.iter().enumerate() {
            ds.injected_input2[i] = if *on { 2 } else { 0 };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_is_inert_when_disabled_or_out_of_fight() {
        let mut ds = DebugState::new();
        // disabled: nothing queued
        tick(&mut ds, 0);
        assert!(ds.pending_bus_writes.is_empty());
        // enabled but bare state (all reads 0 → not in fight): only the
        // credits top-up may fire on frame 0 — and it writes nothing because
        // there is no writable region, so the queue stays empty.
        ds.training.enabled = true;
        tick(&mut ds, 0);
        assert!(ds.pending_bus_writes.is_empty());
        assert_eq!(ds.injected_input2, [0u16; 12]);
    }
}
