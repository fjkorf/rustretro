//! The in-fight controllable gate — ONE shared predicate evaluated by the
//! training enforcer, the Lua `game.controllable()` binding, and (in spirit)
//! the recorder's composite. See docs/game-profiles.md for the condition
//! vocabulary and semantics: byte_zero / word_zero / word_masked_not_all / health_in_range /
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
        GateCond::WordMaskedNotAll { global, mask } => {
            let m = mask.0 as u16;
            rd16(ds, ga(global), little) & m != m
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// MK2 arcade's `screen_state` is a BITFIELD, not an enum: 2-human play
    /// sets 0x100 plus varying low bits (260 and 276 both observed live,
    /// in-fight), while menus set 0x02 (attract 259/263, char select 262).
    /// Enumerating in-fight values broke on the third one and left the
    /// training dummy frozen mid-session — this pins the masked rule against
    /// every phase value recorded in mk2.md.
    #[test]
    fn mk2_masked_screen_state_separates_fights_from_menus() {
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        let mut ds = DebugState::new();
        assert!(ds.install_bus_window(crate::debug::BusWindowCfg {
            name: "wram-gate-test".into(),
            addr: 0x0,
            len: 0x10000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        // Both fighters alive so only screen_state decides.
        let hoff = p.field_off("health").unwrap().0;
        assert!(ds.write_addr((p.block1() + hoff) as usize, 1, 100));
        assert!(ds.write_addr((p.block2() + hoff) as usize, 1, 90));
        let scr = p.global("screen_state").unwrap() as usize;

        for (value, in_fight) in [
            (0u32, true),    // 1P fight / post-KO
            (257, true),     // 2-human fight
            (259, true),     // 2-human fight — LIVE (broke the bit-1 rule)
            (260, true),     // 2-human fight
            (276, true),     // 2-human fight
            (262, false),    // char select / ladder / bios
            (263, false),    // attract
        ] {
            assert!(ds.write_addr(scr, 2, value));
            assert_eq!(
                eval_gate(&ds, &p),
                in_fight,
                "screen_state {value} (0x{value:X}) should read in_fight={in_fight}"
            );
        }
    }
}
