//! Game profiles: per-game knowledge as data instead of compiled constants.
//!
//! Two-tier schema (see docs/game-profiles.md for the design contract):
//!   library/<game>/family.json           — port-independent vocabulary:
//!     roster, move/attack class lists, block style. Shared by every port of
//!     the game and by trained models (meta.json carries family+port).
//!   library/<game>/<game>.profile.json   — port binding: core identity +
//!     capability prerequisites, memory map (blocks, fighter field offsets,
//!     named globals), the controllable-gate condition list, enforcement
//!     values, stage/opponent selector, feature calibration, attack chords.
//!
//! The profile is loaded ONCE at startup (`init`, from `--game <dir>`,
//! default `library/asurabld`) into a process-wide `OnceLock`; call sites
//! use `profile::current()`. This deliberately mirrors how the previous
//! compiled constants behaved (one game per process) while making the game
//! swappable at launch. The Python side reads the SAME JSON files
//! (`shadow_train.profile`), which is what keeps the Rust runner and the
//! Python trainer describing one reality — the successor to the old
//! "hand-kept in four places" rule: now there is one place, and it is data.
//!
//! Addresses serialize as hex strings ("0x403798") for legibility, matching
//! the busmap sidecar convention.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

// ── family.json ─────────────────────────────────────────────────────────────

#[derive(Deserialize, Debug, Clone)]
pub struct Family {
    pub family: String,
    #[serde(default)]
    pub title: String,
    pub roster: Vec<RosterEntry>,
    pub move_classes: Vec<String>,
    pub attack_classes: Vec<String>,
    #[serde(default)]
    pub block: BlockStyle,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RosterEntry {
    pub id: u8,
    pub name: String,
    /// Char-select cursor position (Rights from default); None = not on the
    /// select screen (bosses / hidden characters).
    #[serde(default)]
    pub select_slot: Option<u8>,
    #[serde(default)]
    pub boss: bool,
}

/// How blocking works in this game family. `back_hold` (SF/Asura style) vs
/// a dedicated held button (MK style, named by its attack class).
#[derive(Deserialize, Debug, Clone, Default)]
pub struct BlockStyle {
    #[serde(default = "d_block_style")]
    pub style: String,
    /// For style == "button": which attack-class name is the block button.
    #[serde(default)]
    pub class: Option<String>,
}
fn d_block_style() -> String {
    "back_hold".into()
}

// ── <game>.profile.json ─────────────────────────────────────────────────────

#[derive(Deserialize, Debug, Clone)]
pub struct PortProfile {
    pub family: String,
    pub port: String,
    pub core: CoreInfo,
    #[serde(default)]
    pub requires: Requires,
    pub memory: MemoryMap,
    pub gate: Vec<GateCond>,
    pub enforcement: Enforcement,
    #[serde(default)]
    pub stage_select: Option<StageSelect>,
    /// Feature-scaling constants; keys match `shadow_train.dataset` names.
    pub calibration: BTreeMap<String, f64>,
    /// Attack-class name -> RETRO button names held simultaneously.
    pub attack_chords: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub positions: BTreeMap<String, u32>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct CoreInfo {
    #[serde(default)]
    pub library_name: String,
    pub provenance_game: String,
    pub provenance_core: String,
    /// Logical button -> the RETRO name this core actually responds to
    /// (e.g. MAME cores call coin "select").
    #[serde(default)]
    pub button_names: BTreeMap<String, String>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct Requires {
    #[serde(default)]
    pub memory_regions: bool,
    #[serde(default)]
    pub save_states: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MemoryMap {
    /// "big" (68k) or "little". The read helpers consult this instead of
    /// assuming 68k byte order.
    #[serde(default = "d_endianness")]
    pub endianness: String,
    pub blocks: Blocks,
    pub fighter_fields: Vec<FieldSpec>,
    /// Named global addresses ("round_timer" -> 0x40000A). Gate conditions
    /// and code refer to globals by NAME, never by raw address.
    pub globals: BTreeMap<String, HexAddr>,
}
fn d_endianness() -> String {
    "big".into()
}

#[derive(Deserialize, Debug, Clone)]
pub struct Blocks {
    pub block1: HexAddr,
    pub block2: HexAddr,
    pub stride: HexAddr,
}

#[derive(Deserialize, Debug, Clone)]
pub struct FieldSpec {
    pub name: String,
    pub off: HexAddr,
    /// 1 or 2 bytes (guest order per `endianness`).
    pub size: u8,
}

/// One controllable-gate condition. The vocabulary is fixed and small on
/// purpose — every condition type here is live-verified for at least one
/// game; a game needing logic beyond this vocabulary gets a Lua adapter
/// hook, not a schema extension.
#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GateCond {
    /// u8 at global == 0.
    ByteZero { global: String },
    /// u16 (guest order) at global == 0.
    WordZero { global: String },
    /// BOTH fighters' `health` field in min..=max.
    HealthInRange { min: u8, max: u8 },
    /// u8 at global is nonzero and both BCD nibbles are decimal.
    BcdValidNonzero { global: String },
}

#[derive(Deserialize, Debug, Clone)]
pub struct Enforcement {
    pub health_max: u8,
    pub refill_below: u8,
    /// [seconds byte, subseconds byte] written to round_timer/+1 to hold it.
    pub timer_hold: [u8; 2],
    pub credits_target: u8,
    pub credits_min: u8,
}

#[derive(Deserialize, Debug, Clone)]
pub struct StageSelect {
    pub global: String,
    /// Selector value -> home character id (forced opponent + venue).
    pub value_to_home_char: BTreeMap<String, u8>,
}

// ── hex-string address adapter (busmap convention) ──────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HexAddr(pub u32);

impl<'de> Deserialize<'de> for HexAddr {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            S(String),
            N(u32),
        }
        match Raw::deserialize(d)? {
            Raw::N(n) => Ok(HexAddr(n)),
            Raw::S(s) => {
                let t = s.trim().trim_start_matches("0x").trim_start_matches("0X");
                u32::from_str_radix(t, 16)
                    .map(HexAddr)
                    .map_err(serde::de::Error::custom)
            }
        }
    }
}

// ── the resolved profile ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GameProfile {
    pub dir: PathBuf,
    pub family: Family,
    pub port: PortProfile,
}

impl GameProfile {
    pub fn load(dir: &Path) -> Result<GameProfile, String> {
        let fam_path = dir.join("family.json");
        let family: Family = serde_json::from_str(
            &std::fs::read_to_string(&fam_path)
                .map_err(|e| format!("{}: {e}", fam_path.display()))?,
        )
        .map_err(|e| format!("{}: {e}", fam_path.display()))?;

        // Port profile: <dirname>.profile.json, else the single *.profile.json.
        let stem = dir.file_name().map(|s| s.to_string_lossy().into_owned());
        let mut prof_path = stem
            .as_deref()
            .map(|s| dir.join(format!("{s}.profile.json")))
            .filter(|p| p.is_file());
        if prof_path.is_none() {
            if let Ok(entries) = std::fs::read_dir(dir) {
                prof_path = entries
                    .flatten()
                    .map(|e| e.path())
                    .find(|p| p.to_string_lossy().ends_with(".profile.json"));
            }
        }
        let prof_path = prof_path
            .ok_or_else(|| format!("{}: no *.profile.json found", dir.display()))?;
        let port: PortProfile = serde_json::from_str(
            &std::fs::read_to_string(&prof_path)
                .map_err(|e| format!("{}: {e}", prof_path.display()))?,
        )
        .map_err(|e| format!("{}: {e}", prof_path.display()))?;

        if port.family != family.family {
            return Err(format!(
                "profile family '{}' != family.json '{}'",
                port.family, family.family
            ));
        }
        // Every gate/stage global must resolve; every chord class must exist.
        for cond in &port.gate {
            if let Some(g) = cond.global_name() {
                if !port.memory.globals.contains_key(g) {
                    return Err(format!("gate condition names unknown global '{g}'"));
                }
            }
        }
        for class in port.attack_chords.keys() {
            if !family.attack_classes.iter().any(|c| c == class) {
                return Err(format!("attack_chords names unknown class '{class}'"));
            }
        }
        Ok(GameProfile { dir: dir.to_path_buf(), family, port })
    }

    // ── convenience accessors (the API call sites use) ──────────────────

    pub fn global(&self, name: &str) -> Option<u32> {
        self.port.memory.globals.get(name).map(|a| a.0)
    }

    pub fn block1(&self) -> u32 {
        self.port.memory.blocks.block1.0
    }

    pub fn block2(&self) -> u32 {
        self.port.memory.blocks.block2.0
    }

    pub fn field_off(&self, name: &str) -> Option<(u32, u8)> {
        self.port
            .memory
            .fighter_fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| (f.off.0, f.size))
    }

    pub fn char_name(&self, id: u8) -> String {
        self.family
            .roster
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| format!("c{id}"))
    }

    pub fn matchup_slug(&self, me: u8, opp: u8) -> String {
        format!("{}-vs-{}", self.char_name(me), self.char_name(opp))
    }

    /// Selector value to freeze to fight `opp` next (None if no value).
    pub fn stage_value_for_opponent(&self, opp: u8) -> Option<u8> {
        let ss = self.port.stage_select.as_ref()?;
        ss.value_to_home_char
            .iter()
            .find(|(_, home)| **home == opp)
            .and_then(|(v, _)| v.parse().ok())
    }

    pub fn opponent_for_stage_value(&self, v: u8) -> Option<u8> {
        let ss = self.port.stage_select.as_ref()?;
        ss.value_to_home_char.get(&v.to_string()).copied()
    }

    pub fn calibration(&self, key: &str) -> Option<f64> {
        self.port.calibration.get(key).copied()
    }
}

impl GateCond {
    pub fn global_name(&self) -> Option<&str> {
        match self {
            GateCond::ByteZero { global }
            | GateCond::WordZero { global }
            | GateCond::BcdValidNonzero { global } => Some(global),
            GateCond::HealthInRange { .. } => None,
        }
    }
}

// ── process-wide instance ───────────────────────────────────────────────────

static CURRENT: OnceLock<GameProfile> = OnceLock::new();

/// Load and install the process profile. Call once at startup, before any
/// consumer; a second call is an error (one game per process by design).
pub fn init(dir: &Path) -> Result<&'static GameProfile, String> {
    let p = GameProfile::load(dir)?;
    CURRENT
        .set(p)
        .map_err(|_| "profile::init called twice".to_string())?;
    Ok(CURRENT.get().unwrap())
}

/// The loaded profile. Panics if `init` has not run — that is a startup
/// wiring bug, not a runtime condition. Tests use `init_for_tests`.
pub fn current() -> &'static GameProfile {
    CURRENT.get().expect("profile::init not called")
}

/// Test helper: install the asurabld profile if nothing is loaded yet
/// (idempotent — safe under the multi-threaded test runner).
#[cfg(test)]
pub fn init_for_tests() -> &'static GameProfile {
    if CURRENT.get().is_none() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("library/asurabld");
        let _ = CURRENT.set(GameProfile::load(&dir).expect("asurabld profile parses"));
    }
    CURRENT.get().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_asurabld_profile_parses_and_matches_the_old_constants() {
        let p = init_for_tests();
        assert_eq!(p.family.family, "asurabld");
        assert_eq!(p.port.port, "arcade");
        // The values the compiled constants used to hold.
        assert_eq!(p.block1(), 0x403798);
        assert_eq!(p.block2(), 0x40454C);
        assert_eq!(p.port.memory.blocks.stride.0, 0xDB4);
        assert_eq!(p.global("round_timer"), Some(0x40000A));
        assert_eq!(p.global("char_select"), Some(0x400006));
        assert_eq!(p.global("credits"), Some(0x40655D));
        assert_eq!(p.field_off("health"), Some((0x177, 1)));
        assert_eq!(p.field_off("char_id"), Some((0x639, 1)));
        assert_eq!(p.char_name(1), "goat");
        assert_eq!(p.char_name(9), "sgeist");
        assert_eq!(p.char_name(11), "c11");
        assert_eq!(p.matchup_slug(1, 7), "goat-vs-rosemary");
        // Stage selector round-trips like record.rs's tables.
        assert_eq!(p.stage_value_for_opponent(7), Some(5));
        assert_eq!(p.opponent_for_stage_value(9), Some(9));
        assert_eq!(p.stage_value_for_opponent(3), None); // footee
        // Gate: six conditions, v3 (char_select present).
        assert_eq!(p.port.gate.len(), 6);
        assert!(p.port.gate.iter().any(|c| c.global_name() == Some("char_select")));
        // Chords cover every non-None attack class.
        for class in p.family.attack_classes.iter().filter(|c| *c != "None") {
            assert!(p.port.attack_chords.contains_key(class), "{class} chord missing");
        }
        assert_eq!(p.port.enforcement.health_max, 0xEF);
        assert_eq!(p.port.enforcement.timer_hold, [0x85, 0x03]);
        assert_eq!(p.calibration("GROUND_Y"), Some(216.0));
    }
}
