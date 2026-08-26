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
use crate::profile::GameProfile;

/// Values resolved from the loaded `GameProfile` once per `tick` call — kept
/// as plain locals rather than a cached/static struct so the profile stays
/// hot-swappable-in-theory and the hot path stays simple (small `Vec`/`BTreeMap`
/// lookups on a handful of fields, once per emulated frame — cheap).
struct Resolved {
    block1: u32,
    block2: u32,
    health_off: u32,
    health2_off: u32,
    x_off: u32,
    y_off: u32,
    round_timer: u32,
    round_state: u32,
    credits: u32,
    round_over: u32,
    abort: u32,
    match_end: u32,

    health_max: u8,
    refill_below: u8,
    timer_hold: [u8; 2],
    credits_target: u8,
    credits_min: u8,
    reset_x: (u16, u16),
    ground_y: u16,
}

fn resolve(p: &GameProfile) -> Resolved {
    let g = |name: &str| {
        p.global(name)
            .unwrap_or_else(|| panic!("profile missing global '{name}'"))
    };
    let field = |name: &str| {
        p.field_off(name)
            .unwrap_or_else(|| panic!("profile missing fighter field '{name}'"))
            .0
    };
    Resolved {
        block1: p.block1(),
        block2: p.block2(),
        health_off: field("health"),
        health2_off: field("health2"),
        x_off: field("x"),
        y_off: field("y"),
        round_timer: g("round_timer"),
        round_state: g("round_state"),
        credits: g("credits"),
        round_over: g("round_over"),
        abort: g("abort"),
        match_end: g("match_end"),

        health_max: p.port.enforcement.health_max,
        refill_below: p.port.enforcement.refill_below,
        timer_hold: p.port.enforcement.timer_hold,
        credits_target: p.port.enforcement.credits_target,
        credits_min: p.port.enforcement.credits_min,
        reset_x: (
            *p.port.positions.get("round_start_x_left").unwrap_or(&84) as u16,
            *p.port.positions.get("round_start_x_right").unwrap_or(&232) as u16,
        ),
        ground_y: *p.port.positions.get("round_start_y").unwrap_or(&216) as u16,
    }
}

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
fn in_fight(ds: &DebugState, r: &Resolved) -> bool {
    let t = rd8(ds, r.round_timer);
    let healthy = |b: u32| (1..=r.health_max).contains(&rd8(ds, b + r.health_off));
    rd16be(ds, r.round_over) == 0
        && rd16be(ds, r.abort) == 0
        && rd16be(ds, r.match_end) == 0
        && healthy(r.block1)
        && healthy(r.block2)
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
    let r = resolve(crate::profile::current());
    // Credits top-up, checked once a second: Start must always work.
    if frame % 60 == 0 && rd8(ds, r.credits) < r.credits_min {
        wr8(ds, r.credits, r.credits_target);
    }
    if !in_fight(ds, &r) {
        return;
    }
    // Hold the round clock.
    wr8(ds, r.round_timer, r.timer_hold[0]);
    wr8(ds, r.round_timer + 1, r.timer_hold[1]);
    // Health refill: let damage show, never let anyone die.
    if ds.training.refill {
        for base in [r.block1, r.block2] {
            if rd8(ds, base + r.health_off) < r.refill_below {
                wr8(ds, base + r.health_off, r.health_max);
                wr8(ds, base + r.health2_off, r.health_max);
            }
        }
    }
    // One-shots.
    if ds.training.reset_positions {
        ds.training.reset_positions = false;
        let (b1x, b2x) = if rd16be(ds, r.block1 + r.x_off) <= rd16be(ds, r.block2 + r.x_off) {
            r.reset_x
        } else {
            (r.reset_x.1, r.reset_x.0)
        };
        wr16be(ds, r.block1 + r.x_off, b1x);
        wr16be(ds, r.block1 + r.y_off, r.ground_y);
        wr16be(ds, r.block2 + r.x_off, b2x);
        wr16be(ds, r.block2 + r.y_off, r.ground_y);
    }
    if ds.training.finish_round {
        ds.training.finish_round = false;
        wr8(ds, r.round_state, 0);
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
            let x1 = rd16be(ds, r.block1 + r.x_off) as i32;
            let x2 = rd16be(ds, r.block2 + r.x_off) as i32;
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
        crate::profile::init_for_tests();
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
