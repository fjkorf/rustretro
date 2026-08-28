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
//! the recorder and Lua `game.controllable()` evaluate (via `crate::gate::eval_gate`,
//! shared with `lua_engine`). A profile with no gate list (a stub) has no training
//! at all; [`available`]/[`features`] tell the panel what to offer.
//!
//! All writes go through `DebugState::write_addr`: bus-window addresses queue
//! onto the live 68k bus via the Sek write queue; direct-pointer regions
//! (FBNeo System RAM fallback) are written in place. `freeze` does NOT land
//! on the latter (mk2.md) — per-frame re-assertion here is the workaround.

use crate::debug::{DebugState, DummyMode};
use crate::profile::GameProfile;

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
    /// BlockPunish needs a guard AND a contact signal (`hitstun_sources` or
    /// `contact_signal`) — MACRO_ACTIONS §6.
    pub block_punish: bool,
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
        if !self.block_punish {
            m.push("Block-punish dummy");
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
    /// For button-block families (MK): the held RETRO buttons that block,
    /// resolved from `family.block.class` through `attack_chords`.
    block_chord: Option<[bool; 12]>,
    /// The BlockPunish trigger source (MACRO_ACTIONS §6): per-block hitstun
    /// globals where mapped (asurabld), else the port's global contact signal
    /// (MK2 arcade's hit_counter). None → the mode degrades to plain Block.
    contact: Option<Contact>,
    /// Cooldown window: the signal must be quiet this long to re-arm.
    hitstun_window: u64,
}

/// Resolved contact-signal addresses. Change = contact (hit OR blocked hit).
enum Contact {
    /// Per-block globals (`hitstun_sources`): read the DUMMY's block.
    PerBlock(u32, u32),
    /// One global counter for both players (`contact_signal`).
    Global(u32),
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

    let block_chord = if p.family.block.style == "button" {
        p.family.block.class.as_deref().and_then(|class| {
            // An empty chord (block button not yet verified for this port)
            // must not resolve into a hold-nothing "block" — fall through.
            let chord = p.port.attack_chords.get(class).filter(|c| !c.is_empty())?;
            let mut bits = [false; 12];
            for name in chord {
                bits[crate::profile::retro_button_bit(name)? as usize] = true;
            }
            Some(bits)
        })
    } else {
        None
    };

    // `contact_signal` FIRST: it is the purpose-built "was struck" signal.
    // hitstun_sources is a health delta — blind to zero-chip blocked hits
    // (the user-reported "punishes some hits but not others") and disturbed
    // by refill writes — so it is only the fallback.
    let contact = p
        .port
        .contact_signal
        .as_ref()
        .and_then(|cs| match (&cs.field, &cs.global) {
            (Some(f), _) => Some(Contact::PerBlock(
                p.field_addr(1, f)?.0,
                p.field_addr(2, f)?.0,
            )),
            (None, Some(gl)) => g(gl).map(Contact::Global),
            _ => None,
        })
        .or_else(|| {
            p.port.hitstun_sources.as_ref().and_then(|hs| {
                Some(Contact::PerBlock(g(hs.get("block1")?)?, g(hs.get("block2")?)?))
            })
        });

    Some(Resolved {
        little: p.port.memory.endianness == "little",
        refill,
        timer: g("round_timer").map(|a| (a, e.timer_hold)),
        credits: g("credits").map(|a| (a, e.credits_target, e.credits_min)),
        reset,
        finish: g("round_state"),
        x_pair,
        block_chord,
        contact,
        hitstun_window: p.calibration("HITSTUN_RECENT_FRAMES").unwrap_or(20.0) as u64,
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
    let block_dummy = r.block_chord.is_some() || r.x_pair.is_some();
    Some(Features {
        refill: r.refill.is_some(),
        timer_hold: r.timer.is_some(),
        credits: r.credits.is_some(),
        position_reset: r.reset.is_some(),
        finish_round: r.finish.is_some(),
        block_dummy,
        block_punish: block_dummy && r.contact.is_some(),
    })
}

/// The dummy's guard hold and which block it occupies. The dummy is the
/// RIGHT fighter when X is mapped (correct for freshly started VS rounds),
/// else block 2. Button-block families hold the chord; back-hold families
/// hold away (Right, since the dummy is the right fighter); no X and no
/// chord degrades to standing still.
fn guard_hold(ds: &DebugState, r: &Resolved) -> ([bool; 12], u8) {
    let mut block = 2u8;
    if let Some(xp) = r.x_pair {
        let x1 = rd16(ds, xp.p1, r.little) as i32;
        let x2 = rd16(ds, xp.p2, r.little) as i32;
        block = if x1 >= x2 { 1 } else { 2 };
    }
    if let Some(chord) = r.block_chord {
        return (chord, block);
    }
    let mut b = [false; 12];
    if r.x_pair.is_some() {
        b[7] = true; // away from the opponent = Right
    }
    (b, block)
}

/// Run one training-mode frame. Called from `Frontend::run_frame` after the
/// bus-window refresh (reads see this frame's snapshot; writes drain to the
/// live bus next frame).
pub fn tick(ds: &mut DebugState, frame: u64) {
    tick_with(ds, frame, crate::profile::current());
}

/// [`tick`] against an explicit profile — the testable seam (the process
/// profile is a OnceLock, so per-game tick tests pass their own).
fn tick_with(ds: &mut DebugState, frame: u64, p: &GameProfile) {
    if !ds.training.enabled {
        return;
    }
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
    if !crate::gate::eval_gate(ds, p) {
        // A punish macro already in flight keeps playing through SHORT gate
        // closures — MK2 arcade zeroes its in-fight word at the very contact
        // that triggers the punish (hit-freeze; live-observed 2026-08-28), so
        // stalling here would strand every punish two frames in. A closure
        // longer than the grace is a real round end and drops the macro.
        // Nothing else runs while closed: enforcement stays off menus.
        if ds.training.punish_exec.is_some() {
            ds.training.punish_gate_grace += 1;
            let mut bits = None;
            if ds.training.punish_gate_grace > PUNISH_GATE_GRACE {
                ds.training.punish_exec = None;
            } else if ds.training.punish_wait > 0 {
                // Ride out blockstun guarding, then release for a clean press.
                ds.training.punish_wait -= 1;
                bits = Some(if ds.training.punish_wait < PUNISH_RELEASE {
                    [false; 12]
                } else {
                    guard_hold(ds, &r).0
                });
            } else if let Some(ex) = ds.training.punish_exec.as_mut() {
                bits = ex.next(false); // dummy-is-right default; x is ungated reads
                if bits.is_none() {
                    ds.training.punish_exec = None;
                }
            }
            if let Some(bits) = bits {
                for (i, on) in bits.iter().enumerate() {
                    ds.injected_input2[i] = if *on { 2 } else { 0 };
                }
            }
        }
        return;
    }
    ds.training.punish_gate_grace = 0;
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
            let fired: Vec<(usize, u8, Vec<u32>)> = rf
                .sides
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    let h = rd8(ds, s.check);
                    (h < rf.below).then(|| (i, h, s.addrs.clone()))
                })
                .collect();
            for (side, was, addrs) in fired {
                for addr in addrs {
                    wr8(ds, addr, rf.max);
                }
                ds.log(format!("🎯 refill: P{} {was} → {}", side + 1, rf.max));
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
        // Guard per family block style: the chord for button-block families
        // (MK), hold-away for back-hold families (see `guard_hold`).
        DummyMode::Block => Some(guard_hold(ds, &r).0),
        // Guard, and on each guarded contact sample the weighted punish pool
        // (MACRO_ACTIONS §6). No contact signal mapped → plain guarding (the
        // panel greys the mode with the reason).
        DummyMode::BlockPunish => Some(block_punish(ds, frame, p, &r)),
    };
    if let Some(bits) = dummy_bits {
        for (i, on) in bits.iter().enumerate() {
            ds.injected_input2[i] = if *on { 2 } else { 0 };
        }
    }
}

/// Frames between the contact trigger and the macro's first input: the
/// dummy keeps guarding through hit-freeze + its own blockstun, then
/// punishes — inputs played into the freeze are eaten by the game
/// (live-observed on MK2 arcade, 2026-08-28).
/// 26 ≈ hit-freeze (~10) + jab blockstun (~14) + slack: a chord played at
/// +16 was still eaten while a motion whose chord lands at +21 came out —
/// live-calibrated on MK2 arcade 2026-08-28.
const PUNISH_DELAY: u64 = 26;

/// Neutral frames at the tail of the delay: a still-held guard bleeds into
/// the macro's chord (MK's held Block eats attack buttons — live-observed:
/// the slide fires from a clean simultaneous press, not from Block-held +
/// buttons added), so everything is released before the first step.
const PUNISH_RELEASE: u64 = 4;

/// Frames an in-flight punish (delay + macro) may keep running while the
/// gate is closed (MK2 zeroes its in-fight word from the contact frame
/// onward) before it is dropped as a real round end.
const PUNISH_GATE_GRACE: u64 = 60;

/// One BlockPunish frame: guard by default; when the contact signal changes
/// while armed, sample the weighted pool and play the pick through
/// [`crate::macros::MacroExec`] on the dummy port. The cooldown re-arms only
/// after the signal has been quiet ≥ HITSTUN_RECENT_FRAMES (§6), so one
/// blocked string triggers one punish, not one per chip hit.
fn block_punish(ds: &mut DebugState, frame: u64, p: &GameProfile, r: &Resolved) -> [bool; 12] {
    use crate::macros::PunishOption;
    let (guard_bits, dummy_block) = guard_hold(ds, r);
    let Some(contact) = &r.contact else {
        return guard_bits; // no signal mapped — degrade to plain Block
    };
    let sig_addr = match contact {
        Contact::Global(a) => *a,
        Contact::PerBlock(b1, b2) => if dummy_block == 1 { *b1 } else { *b2 },
    };
    let cur = rd8(ds, sig_addr);
    let changed = ds.training.punish_prev_signal.is_some_and(|prev| prev != cur);
    ds.training.punish_prev_signal = Some(cur);
    if changed {
        ds.training.punish_last_change = frame;
    }
    if !ds.training.punish_armed
        && frame.saturating_sub(ds.training.punish_last_change) >= r.hitstun_window
    {
        ds.training.punish_armed = true;
    }
    // The dummy is the right fighter (guard_hold), so its opponent is left.
    // Re-derived every frame — a side switch mid-macro flips "back" with it.
    let opp_right = false;

    // An in-flight punish: guard out the post-contact delay, then play the
    // macro to completion.
    let mut out: Option<[bool; 12]> = None;
    if ds.training.punish_exec.is_some() {
        if ds.training.punish_wait > 0 {
            ds.training.punish_wait -= 1;
            // Guard through blockstun (out = None falls through to the guard
            // hold), then release everything for a clean chord press.
            if ds.training.punish_wait < PUNISH_RELEASE {
                out = Some([false; 12]);
            }
        } else {
            let mut done = false;
            if let Some(ex) = ds.training.punish_exec.as_mut() {
                match ex.next(opp_right) {
                    Some(bits) => out = Some(bits),
                    None => done = true,
                }
            }
            if done {
                ds.training.punish_exec = None;
            }
        }
    }

    if ds.training.punish_exec.is_none()
        && changed
        && ds.training.punish_armed
        && frame >= ds.training.punish_hold_until
    {
        ds.training.punish_armed = false;
        // Never deterministic (§6): wall-clock entropy in the seed.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0);
        let seed = frame ^ ((cur as u64) << 32) ^ nanos;
        let pick = crate::macros::weighted_pick(&ds.training.punish_pool, seed).cloned();
        // Scheduled, not stepped: the macro's first input lands PUNISH_DELAY
        // frames from now, after hit-freeze + blockstun have passed.
        let start = |m: crate::macros::CompiledMacro, ds: &mut DebugState| {
            ds.training.punish_exec = Some(crate::macros::MacroExec::new(m));
            ds.training.punish_wait = PUNISH_DELAY;
        };
        match pick {
            Some(PunishOption::Move(name)) => {
                let char_id = p
                    .field_addr(dummy_block, "char_id")
                    .map(|(addr, _)| p.canon_char_id(rd8(ds, addr)));
                let steps = char_id.and_then(|id| {
                    p.specials_for(id)
                        .into_iter()
                        .find(|(n, _)| *n == name)
                        .map(|(_, s)| s.to_vec())
                });
                match steps.and_then(|s| crate::macros::compile(&name, &s, p).ok()) {
                    Some(m) => {
                        start(m, ds);
                        ds.log(format!("🎯 punish: {name}"));
                        eprintln!("[training] punish: {name}"); // headless-visible twin
                    }
                    // Stale pool (character changed since the panel built it).
                    None => ds.log(format!("🎯 punish: '{name}' not encoded for this character")),
                }
            }
            Some(PunishOption::Attack(class)) => {
                let spec = crate::profile::StepSpec {
                    dirs: Vec::new(),
                    press: vec![class.clone()],
                    frames: 3,
                };
                if let Ok(m) = crate::macros::compile(&class, &[spec], p) {
                    start(m, ds);
                    ds.log(format!("🎯 punish: {class}"));
                    eprintln!("[training] punish: {class}"); // headless-visible twin
                }
            }
            Some(PunishOption::ContinueBlock(n)) => {
                ds.training.punish_hold_until = frame + n as u64;
                ds.log("🎯 punish: continue block".into());
            }
            None => {} // empty/zero-weight pool: just keep guarding
        }
    }
    out.unwrap_or(guard_bits)
}

/// The dummy's CANONICAL char id (its block's `char_id` through `id_map`) —
/// what the panel keys `specials_for` on. None while training is unavailable
/// or the port maps no `char_id`.
pub fn punish_dummy_char(ds: &DebugState) -> Option<u8> {
    let p = crate::profile::current();
    let r = resolve(p)?;
    let (_, block) = guard_hold(ds, &r);
    let (addr, _) = p.field_addr(block, "char_id")?;
    Some(p.canon_char_id(rd8(ds, addr)))
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
        assert!(f.block_punish, "hitstun_sources + x pair → BlockPunish available");
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
        assert!(f.block_punish, "hitstun_sources (HUD health pair) is mapped");
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
        // MK is button-block: the dummy must hold the Block chord (L), not
        // walk backward.
        let chord = r.block_chord.expect("mk2 dummy blocks with a button");
        let held: Vec<usize> = chord.iter().enumerate().filter(|(_, on)| **on).map(|(i, _)| i).collect();
        assert_eq!(held, vec![10], "Block = RETRO L");
    }

    #[test]
    fn genesis_pins_resolve_to_pad_mode_flags() {
        let p = GameProfile::load(Path::new("library/mk2/genesis")).expect("genesis loads");
        let pins = p.resolved_pins();
        assert_eq!(pins, vec![(0xFFF9D1, 1), (0xFFF9D0, 1)],
                   "both 6-button flags pinned on (mk2-genesis.md)");
        // asurabld declares no pins.
        assert!(crate::profile::init_for_tests().resolved_pins().is_empty());
    }

    /// The full §6 loop against the mk2 profile: arm on quiet, trigger on a
    /// hit_counter change while guarding, play the char-aware slide through
    /// MacroExec on the dummy port, return to the guard chord, and stay in
    /// cooldown until the signal goes quiet again.
    #[test]
    fn block_punish_fires_the_slide_on_contact_then_cools_down() {
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        let mut ds = DebugState::new();
        assert!(ds.install_bus_window(crate::debug::BusWindowCfg {
            name: "wram-punish-test".into(),
            addr: 0x0,
            len: 0x10000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        // Open mk2's gate and stage the matchup: dummy (higher X → block2)
        // is Reptile; its contact signal is the block2 hitstun source
        // (p2_health_hud — the HUD accumulator that moves when P2 is struck).
        let hoff = p.field_off("health").unwrap().0;
        let coff = p.field_off("char_id").unwrap().0;
        assert!(ds.write_addr((p.block1() + hoff) as usize, 1, 100));
        assert!(ds.write_addr((p.block2() + hoff) as usize, 1, 90));
        assert!(ds.write_addr((p.block2() + coff) as usize, 1, 9)); // reptile
        assert!(ds.write_addr(p.global("p1_x").unwrap() as usize, 2, 100));
        assert!(ds.write_addr(p.global("p2_x").unwrap() as usize, 2, 200));
        // The dummy is block2 (larger x); mk2 ships no contact_signal, so
        // the trigger falls back to hitstun_sources — block2's HUD health.
        // (A blocking MK2 fighter's struct is otherwise frozen and blocked
        // contact always chips, so the health delta IS the contact event —
        // see mk2.md's contact-signal investigation.)
        let sig = p.global("p2_health_hud").unwrap() as usize;
        assert!(crate::gate::eval_gate(&ds, &p));

        ds.training.enabled = true;
        ds.training.dummy = crate::debug::DummyMode::BlockPunish;
        ds.training.punish_pool =
            vec![(crate::macros::PunishOption::Move("slide".into()), 1)];

        // Quiet frames arm the trigger; meanwhile the dummy holds the guard
        // chord (Block = RETRO L, bit 10).
        for f in 1..=25 {
            tick_with(&mut ds, f, &p);
        }
        assert!(ds.training.punish_armed);
        assert_eq!(ds.injected_input2[10], 2, "guarding while armed");
        assert_eq!(ds.injected_input2[8], 0);

        // Contact: the signal moves while the dummy guards → punish is
        // SCHEDULED (guard held through hit-freeze + blockstun first).
        assert!(ds.write_addr(sig, 1, 1));
        tick_with(&mut ds, 26, &p);
        assert!(ds.training.punish_exec.is_some(), "punish scheduled");
        assert_eq!(ds.training.punish_wait, PUNISH_DELAY);
        assert_eq!(ds.injected_input2[10], 2, "still guarding through the delay");
        assert!(!ds.training.punish_armed, "trigger disarms");

        // Transient gate closure — the scheduled punish must ride it out
        // under grace instead of stalling. 262 = char select/ladder (the
        // documented arcade menu value: bit 0x02 set); the 2-human in-fight
        // values 260/276 have it CLEAR and are legal — see mk2.md's gate
        // revisions.
        let scr = p.global("screen_state").unwrap() as usize;
        assert!(ds.write_addr(scr, 2, 262));
        assert!(!crate::gate::eval_gate(&ds, &p));
        let mut f = 27;
        tick_with(&mut ds, f, &p);
        assert_eq!(ds.injected_input2[10], 2, "guard held while riding out blockstun");
        f += 1;
        while ds.training.punish_wait > 0 {
            tick_with(&mut ds, f, &p);
            f += 1;
        }
        // The release tail dropped everything for a clean chord press.
        assert_eq!(ds.injected_input2[10], 0, "guard released before the chord");
        // Delay drained: the slide plays under the closed gate. Frame 1:
        // back (dummy is the RIGHT fighter, opponent left → back = Right,
        // bit 7) + LK (a, 8) + LP (b, 0) + Block (l, 10 — part of the chord).
        tick_with(&mut ds, f, &p);
        assert_eq!(ds.injected_input2[7], 2);
        assert_eq!(ds.injected_input2[8], 2);
        assert_eq!(ds.injected_input2[0], 2);
        assert_eq!(ds.injected_input2[10], 2, "Block is part of the verified chord");
        for i in 1..=8 {
            tick_with(&mut ds, f + i, &p);
        }
        assert!(ds.training.punish_exec.is_none(), "macro finished under grace");
        f += 9;

        // Gate reopens: the guard chord returns.
        assert!(ds.write_addr(scr, 2, 0));
        tick_with(&mut ds, f, &p);
        assert_eq!(ds.injected_input2[10], 2, "back to guarding");
        assert_eq!(ds.injected_input2[8], 0);

        // Another change inside the quiet window must NOT re-trigger.
        assert!(ds.write_addr(sig, 1, 2));
        tick_with(&mut ds, f + 1, &p);
        assert_eq!(ds.injected_input2[10], 2, "cooldown holds the guard");
        assert!(!ds.training.punish_armed);
    }

    #[test]
    fn asurabld_blocks_by_holding_back_not_a_chord() {
        let p = crate::profile::init_for_tests();
        let r = resolve(p).unwrap();
        assert!(r.block_chord.is_none(), "back_hold family must use X-relative block");
        assert!(r.x_pair.is_some());
    }
}
