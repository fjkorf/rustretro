//! The in-fight controllable gate — ONE shared predicate evaluated by the
//! training enforcer, the Lua `game.controllable()` binding, and (in spirit)
//! the recorder's composite. See docs/game-profiles.md for the condition
//! vocabulary and semantics: byte_zero / word_zero / word_in / health_in_range /
//! bcd_valid_nonzero. Reads go through the same `DebugState::read_addr` path
//! as every other binding; out-of-map reads collapse to 0, matching the
//! recorder's `unwrap_or(0)` semantics. 16-bit reads honor the profile's
//! `memory.endianness` per the contract.

use crate::debug::DebugState;
use crate::profile::{GameProfile, GateCond};

fn rd8(ds: &DebugState, addr: u32) -> u8 {
    ds.read_addr(addr as usize, 1).unwrap_or(0) as u8
}

fn rd16(ds: &DebugState, addr: u32, little: bool) -> u16 {
    let v = ds.read_addr(addr as usize, 2).unwrap_or(0) as u16;
    if little { v } else { v.swap_bytes() }
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
        GateCond::WordIn { global, values } => {
            values.contains(&rd16(ds, ga(global), little))
        }
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
