//! Training mode (shadow PLAN Wave 2b): an infinite, resettable practice
//! fight for demonstration recording.
//!
//! Enabled with `--training`/F5. Each emulated frame, [`tick`] applies every
//! enforcement the loaded profile can support — **per-feature, not
//! all-or-nothing**, so a partially-mapped game (MK2) gets health refill and
//! the dummy while its unmapped enforcements (timer hold, credits, position
//! reset) decline individually:
//! - **credits topped up** so Start always works (`credits` global),
//! - **round timer held** — no timeouts (`round_timer` global),
//! - **health refill**: below the threshold every mapped health byte is
//!   rewritten to max — fighter-block health/health2 plus any per-player HUD
//!   accumulator globals (`p1_health_hud`/`p2_health_hud`; MK2 damages all
//!   four independently) — so damage/hitstun stay visible but nobody is ever
//!   KO'd (toggle with F3),
//! - **dummy control**: a preset drives controller port 1 (F1 cycles
//!   Free / Stand / Crouch / Jump / Block) — Block holds away from the other
//!   fighter using live X positions (fighter-field `x` or `p1_x`/`p2_x`
//!   globals),
//! - **position reset** (F2, needs X source + explicit `positions`) and
//!   **finish round now** (F4, needs `round_state`) one-shots.
//!
//! The in-fight gate is the profile's `gate` condition list — the SAME gate
//! the recorder and Lua `game.controllable()` evaluate ([`eval_gate`], shared
//! with `lua_engine`). A profile with no gate list (a stub) has no training
//! at all; [`available`]/[`features`] tell the panel what to offer.
//!
//! All writes go through `DebugState::write_addr`: bus-window addresses queue
//! onto the live 68k bus via the Sek write queue; direct-pointer regions
//! (FBNeo System RAM fallback) are written in place. `freeze` does NOT land
//! on the latter (mk2.md) — per-frame re-assertion here is the workaround.

use crate::debug::{DebugState, DummyMode};
use crate::profile::{GameProfile, GateCond};

/// Which training features the loaded profile supports — the panel uses this
/// to disable individual controls with honest hints instead of hiding the
/// whole mode behind one all-or-nothing message.
pub struct Features {
    pub refill: bool,
    pub timer_hold: bool,
    pub credits: bool,
    pub position_reset: bool,
    pub finish_round: bool,
    pub block_dummy: bool,
}

impl Features {
    /// Feature labels that are NOT mapped for this game (panel hint line).
    pub fn missing(&self) -> Vec<&'static str> {
        let mut m = Vec::new();
        if !self.refill {
            m.push("health refill");
        }
        if !self.timer_hold {
            m.push("timer hold");
        }
        if !self.credits {
            m.push("credits top-up");
        }
        if !self.position_reset {
            m.push("position reset");
        }
        if !self.finish_round {
            m.push("finish round");
        }
        if !self.block_dummy {
            m.push("Block dummy");
        }
        m
    }
}

/// One fighter's refill writes: `check` is the authoritative struct health
/// consulted against the threshold; `addrs` is every byte rewritten to max
/// (struct health, health2 if mapped, per-player HUD accumulator if mapped).
struct RefillSide {
    check: u32,
    addrs: Vec<u32>,
}

struct Refill {
    sides: [RefillSide; 2],
    max: u8,
    below: u8,
}

/// Resolved absolute X addresses for the two fighters (block field or globals).
#[derive(Clone, Copy)]
struct XPair {
    p1: u32,
    p2: u32,
}

struct Reset {
    x: XPair,
    left: u16,
    right: u16,
    /// (block1 y, block2 y, ground) — only when the profile maps Y.
    y: Option<(u32, u32, u16)>,
}

/// Values resolved from the loaded `GameProfile` once per `tick` call — kept
/// as plain locals rather than a cached/static struct so the profile stays
/// hot-swappable-in-theory and the hot path stays simple (small `Vec`/`BTreeMap`
/// lookups on a handful of fields, once per emulated frame — cheap).
struct Resolved {
    little: bool,
    refill: Option<Refill>,
    timer: Option<(u32, [u8; 2])>,
    credits: Option<(u32, u8, u8)>,
    reset: Option<Reset>,
    finish: Option<u32>,
    x_pair: Option<XPair>,
}

fn resolve(p: &GameProfile) -> Option<Resolved> {
    // No gate list = no way to know when we're in a fight — training as a
    // whole must no-op rather than enforce on menus (same class as the
    // QA-found Record crash: stub profiles refuse softly).
    if p.port.gate.is_empty() {
        return None;
    }
    let g = |name: &str| p.global(name);
    let field = |name: &str| p.field_off(name).map(|(off, _)| off);
    let e = &p.port.enforcement;

    let x_pair = field("x")
        .map(|off| XPair { p1: p.block1() + off, p2: p.block2() + off })
        .or_else(|| Some(XPair { p1: g("p1_x")?, p2: g("p2_x")? }));

    let refill = field("health").map(|off| {
        let side = |base: u32, hud: &str| {
            let mut addrs = vec![base + off];
            if let Some(h2) = field("health2") {
                addrs.push(base + h2);
            }
            if let Some(a) = g(hud) {
                addrs.push(a);
            }
            RefillSide { check: base + off, addrs }
        };
        Refill {
            sides: [side(p.block1(), "p1_health_hud"), side(p.block2(), "p2_health_hud")],
            max: e.health_max,
            below: e.refill_below,
        }
    });

    let reset = (|| {
        let x = x_pair?;
        // Explicit positions required — no silent asurabld-shaped defaults
        // teleporting an unmapped game to nonsense coordinates.
        let left = *p.port.positions.get("round_start_x_left")? as u16;
        let right = *p.port.positions.get("round_start_x_right")? as u16;
        let y = (|| {
            let off = field("y")?;
            let ground = *p.port.positions.get("round_start_y")? as u16;
            Some((p.block1() + off, p.block2() + off, ground))
        })();
        Some(Reset { x, left, right, y })
    })();

    Some(Resolved {
        little: p.port.memory.endianness == "little",
        refill,
        timer: g("round_timer").map(|a| (a, e.timer_hold)),
        credits: g("credits").map(|a| (a, e.credits_target, e.credits_min)),
        reset,
        finish: g("round_state"),
        x_pair,
    })
}

fn rd8(ds: &DebugState, addr: u32) -> u8 {
    ds.read_addr(addr as usize, 1).unwrap_or(0) as u8
}

fn rd16(ds: &DebugState, addr: u32, little: bool) -> u16 {
    let v = ds.read_addr(addr as usize, 2).unwrap_or(0) as u16;
    if little { v } else { v.swap_bytes() }
}

fn wr8(ds: &mut DebugState, addr: u32, v: u8) {
    let _ = ds.write_addr(addr as usize, 1, v as u32);
}

fn wr16(ds: &mut DebugState, addr: u32, v: u16, little: bool) {
    // write_addr takes little-endian value bytes to ascending addresses; swap
    // for big-endian guests (68k) so the guest reads `v`.
    let v = if little { v } else { v.swap_bytes() };
    let _ = ds.write_addr(addr as usize, 2, v as u32);
}

/// Evaluate the loaded profile's controllable-gate condition list against the
/// live snapshot — the ONE in-fight gate shared by training enforcement, the
/// Lua `game.controllable()` binding, and (in spirit) the recorder's
/// composite; a lua_engine unit test locks Lua and the recorder together.
/// The condition vocabulary is closed (docs/game-profiles.md): byte_zero /
/// word_zero / health_in_range / bcd_valid_nonzero. Reads go through the same
/// `DebugState::read_addr` path as every other binding; out-of-map reads
/// collapse to 0, matching the recorder's `unwrap_or(0)` semantics. 16-bit
/// reads honor the profile's `memory.endianness` per the contract.
pub(crate) fn eval_gate(ds: &DebugState, p: &GameProfile) -> bool {
    let little = p.port.memory.endianness == "little";
    // Profile load validates that every gate global resolves, so a miss here
    // is impossible in practice; 0 keeps the closure total anyway.
    let ga = |name: &str| p.global(name).unwrap_or(0);
    p.port.gate.iter().all(|cond| match cond {
        GateCond::ByteZero { global } => rd8(ds, ga(global)) == 0,
        GateCond::WordZero { global } => rd16(ds, ga(global), little) == 0,
        GateCond::HealthInRange { min, max } => {
            let Some((off, _size)) = p.field_off("health") else {
                return false;
            };
            let h1 = rd8(ds, p.block1().wrapping_add(off));
            let h2 = rd8(ds, p.block2().wrapping_add(off));
            (*min..=*max).contains(&h1) && (*min..=*max).contains(&h2)
        }
        GateCond::BcdValidNonzero { global } => {
            let t = rd8(ds, ga(global));
            t != 0 && (t >> 4) <= 9 && (t & 0xF) <= 9
        }
    })
}

/// Whether the loaded profile supports training at all (has an in-fight
/// gate). Per-feature detail comes from [`features`].
pub fn available() -> bool {
    resolve(crate::profile::current()).is_some()
}

/// Per-feature availability for the loaded profile — `None` when training is
/// unavailable entirely (no gate list).
pub fn features() -> Option<Features> {
    features_of(crate::profile::current())
}

fn features_of(p: &GameProfile) -> Option<Features> {
    let r = resolve(p)?;
    Some(Features {
        refill: r.refill.is_some(),
        timer_hold: r.timer.is_some(),
        credits: r.credits.is_some(),
        position_reset: r.reset.is_some(),
        finish_round: r.finish.is_some(),
        block_dummy: r.x_pair.is_some(),
    })
}

/// Run one training-mode frame. Called from `Frontend::run_frame` after the
/// bus-window refresh (reads see this frame's snapshot; writes drain to the
/// live bus next frame).
pub fn tick(ds: &mut DebugState, frame: u64) {
    if !ds.training.enabled {
        return;
    }
    let p = crate::profile::current();
    let Some(r) = resolve(p) else {
        // Stub profile: no in-fight gate mapped — refuse softly, once.
        ds.training.enabled = false;
        ds.log("🎯 Training unavailable: this game's profile has no in-fight gate yet".into());
        eprintln!("[training] unavailable: profile has no gate conditions (stub) — disabled");
        return;
    };
    // Credits top-up, checked once a second: Start must always work.
    if let Some((addr, target, min)) = r.credits {
        if frame % 60 == 0 && rd8(ds, addr) < min {
            wr8(ds, addr, target);
        }
    }
    if !eval_gate(ds, p) {
        return;
    }
    // Hold the round clock.
    if let Some((addr, hold)) = r.timer {
        wr8(ds, addr, hold[0]);
        wr8(ds, addr + 1, hold[1]);
    }
    // Health refill: let damage show, never let anyone die. Every mapped
    // accumulator for the refilled fighter is rewritten (MK2's HUD pair
    // tracks damage independently of the struct byte — mk2.md).
    if ds.training.refill {
        if let Some(rf) = &r.refill {
            let writes: Vec<u32> = rf
                .sides
                .iter()
                .filter(|s| rd8(ds, s.check) < rf.below)
                .flat_map(|s| s.addrs.iter().copied())
                .collect();
            for addr in writes {
                wr8(ds, addr, rf.max);
            }
        }
    }
    // One-shots — each declines with a log line when its map is missing.
    if ds.training.reset_positions {
        ds.training.reset_positions = false;
        match &r.reset {
            Some(rs) => {
                let (x1, x2) = (rd16(ds, rs.x.p1, r.little), rd16(ds, rs.x.p2, r.little));
                let (b1x, b2x) =
                    if x1 <= x2 { (rs.left, rs.right) } else { (rs.right, rs.left) };
                wr16(ds, rs.x.p1, b1x, r.little);
                wr16(ds, rs.x.p2, b2x, r.little);
                if let Some((y1, y2, ground)) = rs.y {
                    wr16(ds, y1, ground, r.little);
                    wr16(ds, y2, ground, r.little);
                }
            }
            None => ds.log("🎯 Position reset: not mapped for this game".into()),
        }
    }
    if ds.training.finish_round {
        ds.training.finish_round = false;
        match r.finish {
            Some(addr) => wr8(ds, addr, 0),
            None => ds.log("🎯 Finish round: not mapped for this game".into()),
        }
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
        DummyMode::Block => match r.x_pair {
            // Hold away from the other fighter. The dummy is port 1; without a
            // resolved port→block map, treat the RIGHT fighter as the dummy
            // (P2 side) — correct for freshly started VS rounds.
            Some(xp) => {
                let x1 = rd16(ds, xp.p1, r.little) as i32;
                let x2 = rd16(ds, xp.p2, r.little) as i32;
                let (dummy_x, other_x) = if x1 >= x2 { (x1, x2) } else { (x2, x1) };
                let mut b = [false; 12];
                if dummy_x >= other_x {
                    b[7] = true; // opponent to the left → hold Right (away)
                } else {
                    b[6] = true;
                }
                Some(b)
            }
            // No X source mapped: fall back to standing still.
            None => Some([false; 12]),
        },
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
    use std::path::Path;

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

    #[test]
    fn asurabld_supports_every_feature() {
        let p = crate::profile::init_for_tests();
        let f = features_of(p).expect("asurabld must be training-available");
        assert!(f.refill && f.timer_hold && f.credits);
        assert!(f.position_reset && f.finish_round && f.block_dummy);
        assert!(f.missing().is_empty());
    }

    #[test]
    fn mk2_degrades_per_feature() {
        // MK2's map is partial by honesty (mk2.md): gate + health + world X
        // exist; timer store, credits (CMOS), Y, and round_state don't.
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        let f = features_of(&p).expect("mk2 must be training-available (has a gate)");
        assert!(f.refill, "health field + HUD pair are mapped");
        assert!(f.block_dummy, "p1_x/p2_x globals are mapped");
        assert!(!f.timer_hold && !f.credits && !f.position_reset && !f.finish_round);
        assert_eq!(
            f.missing(),
            vec!["timer hold", "credits top-up", "position reset", "finish round"]
        );
        // And the refill spec includes all four MK2 health bytes.
        let r = resolve(&p).unwrap();
        let rf = r.refill.unwrap();
        let all: Vec<u32> = rf.sides.iter().flat_map(|s| s.addrs.iter().copied()).collect();
        assert_eq!(all.len(), 4, "struct pair + HUD pair: {all:x?}");
    }
}
