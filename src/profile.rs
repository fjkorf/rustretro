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

// Some schema fields are contract surface read by the Python/Lua sides or
// by serde validation only — not (yet) by Rust code. That is by design.
#[allow(dead_code)]
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
    /// Family-level move vocabulary (shadow/MACRO_ACTIONS.md §1), keyed by
    /// canonical roster NAME. A character absent here simply has no specials;
    /// an absent map keeps the whole macro layer off (asurabld this phase).
    #[serde(default)]
    pub moves: BTreeMap<String, Vec<MoveDef>>,
}

/// One named family-level move intent. Tags are open strings; "special"
/// marks label-space membership on the Python side.
#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
pub struct MoveDef {
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[allow(dead_code)]
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
#[allow(dead_code)]
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
    /// Where hitstun evidence lives: block name ("block1"/"block2") -> the global
    /// whose recent change indicates hitstun for that block's fighter.
    #[serde(default)]
    pub hitstun_sources: Option<BTreeMap<String, String>>,
    /// Raw RAM char id -> canonical roster id. Absent map or absent key = identity.
    /// Keys are decimal strings (JSON object constraint). Values must exist in family roster.
    #[serde(default)]
    pub id_map: Option<BTreeMap<String, u8>>,
    /// RAM values the app holds for the whole session (re-asserted ~1 Hz),
    /// independent of training mode and the gate — for settings the game
    /// keeps in volatile RAM that must not silently reset on a cold boot
    /// (MK2 Genesis: the per-port 6-button pad flags). `freeze` can't do
    /// this on direct-pointer regions; periodic writes are the mechanism.
    #[serde(default)]
    pub pins: Vec<Pin>,
    /// Port-level special-move encodings (shadow/MACRO_ACTIONS.md §2):
    /// character name -> move name -> ordered macro steps. OMISSION IS
    /// MEANINGFUL — a port that lacks a move omits its entry and every
    /// consumer offers only what the port encodes.
    #[serde(default)]
    pub special_inputs: BTreeMap<String, BTreeMap<String, Vec<StepSpec>>>,
    /// Global "somebody was just struck" signal (hit OR block), used by the
    /// block-punish dummy when `hitstun_sources` is absent (MK2 arcade's
    /// hit_counter). Neither mapped → BlockPunish degrades per-feature.
    #[serde(default)]
    pub contact_signal: Option<ContactSignal>,
}

/// The contact-signal declaration: something whose CHANGE means "this
/// fighter was struck (hit OR blocked)". Exactly one source:
/// - `field`: a per-fighter field name, resolved per block — PREFERRED,
///   because it is per-victim by construction and (MK2's `action_counter`)
///   fires on zero-chip blocked contact, which a health delta cannot see.
/// - `global`: one address shared by both fighters (weaker: usually
///   victim-asymmetric, as MK2's hit_counter turned out to be).
#[derive(Deserialize, Debug, Clone)]
pub struct ContactSignal {
    #[serde(default)]
    pub field: Option<String>,
    #[serde(default)]
    pub global: Option<String>,
}

/// One macro step (MACRO_ACTIONS §2): held SEMANTIC directions, attack
/// CLASSES pressed together, and the executor's hold length in frames.
#[derive(Deserialize, Debug, Clone)]
pub struct StepSpec {
    #[serde(default)]
    pub dirs: Vec<String>,
    #[serde(default)]
    pub press: Vec<String>,
    #[serde(default = "d_step_frames")]
    pub frames: u8,
}
fn d_step_frames() -> u8 {
    3
}

/// One pinned RAM value: a named global asserted to `value` for the session.
#[derive(Deserialize, Debug, Clone)]
pub struct Pin {
    pub global: String,
    pub value: u8,
}

#[allow(dead_code)]
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

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone, Default)]
pub struct Requires {
    #[serde(default)]
    pub memory_regions: bool,
    #[serde(default)]
    pub save_states: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MemoryMap {
    /// Main CPU family: "m68k" (default) or "tms34010". Gates the per-frame
    /// Sek debug capture — FBNeo exports the Sek symbols for EVERY game, so
    /// calling them on a non-68k driver dereferences an uninitialized CPU
    /// context and segfaults (probe-verified on mk2, 2026-08-26).
    #[serde(default = "d_cpu")]
    pub cpu: String,
    /// "big" (68k) or "little". The read helpers consult this instead of
    /// assuming 68k byte order.
    #[serde(default = "d_endianness")]
    pub endianness: String,
    pub blocks: Blocks,
    pub fighter_fields: Vec<FieldSpec>,
    /// Named global addresses ("round_timer" -> 0x40000A). Gate conditions
    /// and code refer to globals by NAME, never by raw address.
    pub globals: BTreeMap<String, HexAddr>,
    /// Extra per-frame sampled globals beyond those in gate conditions.
    /// Each entry names a global and specifies its read size (1 or 2 bytes).
    #[serde(default)]
    pub record_globals: Vec<RecordGlobal>,
}
fn d_endianness() -> String {
    "big".into()
}
fn d_cpu() -> String {
    "m68k".into()
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
pub struct Blocks {
    pub block1: HexAddr,
    pub block2: HexAddr,
    pub stride: HexAddr,
}

#[derive(Deserialize, Debug, Clone)]
pub struct FieldSpec {
    pub name: String,
    /// Offset from the fighter block base — the common case. Exactly one of
    /// `off` / `globals` must be present (validated at load).
    #[serde(default)]
    pub off: Option<HexAddr>,
    /// Global-sourced variant: a per-block pair of named globals, for values
    /// that live OUTSIDE the fighter structs (MK2 arcade's world X sits in a
    /// separate object array — `p1_x`/`p2_x` globals). Consumers see a normal
    /// named field either way.
    #[serde(default)]
    pub globals: Option<BlockGlobals>,
    /// 1 or 2 bytes (guest order per `endianness`).
    pub size: u8,
}

/// The per-block global names backing a global-sourced fighter field.
#[derive(Deserialize, Debug, Clone)]
pub struct BlockGlobals {
    pub block1: String,
    pub block2: String,
}

/// Entry in `memory.record_globals`: a global name and read size for per-frame sampling.
#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
pub struct RecordGlobal {
    pub name: String,
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
    /// `u16 (guest order) at global & mask != mask` — "these bits are not
    /// ALL set". For phase words that are BITFIELDS rather than enums: MK2
    /// arcade's screen_state carries match-type bits (2-human play sets
    /// 0x100 plus varying low bits; 0/257/259/260/276 all observed IN a
    /// fight) and only the COMBINATION 0x06 marks a menu (char select 262,
    /// attract 263). Enumerating in-fight values, then testing a single
    /// bit, were both whack-a-mole that later observations broke — see
    /// mk2.md's three gate revisions.
    WordMaskedNotAll { global: String, mask: HexAddr },
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
    /// Load a game profile from a path. The path may be:
    /// 1. A directory containing family.json + port profile(s):
    ///    - tries `<dirname>.profile.json` first (legacy default)
    ///    - else the single `*.profile.json` in the directory
    ///    - errors if none found or multiple without a default
    /// 2. A path like `dir/port_selector` (dir exists, file does not):
    ///    - family dir = parent
    ///    - tries `<parent>/<port_selector>.profile.json`
    ///    - else scans for a profile with `"port": "<port_selector>"`
    ///    - exactly one match wins; else error
    pub fn load(dir: &Path) -> Result<GameProfile, String> {
        // Determine family dir and profile path.
        let (fam_dir, prof_path) = Self::resolve_game_dir(dir)?;

        let fam_path = fam_dir.join("family.json");
        let family: Family = serde_json::from_str(
            &std::fs::read_to_string(&fam_path)
                .map_err(|e| format!("{}: {e}", fam_path.display()))?,
        )
        .map_err(|e| format!("{}: {e}", fam_path.display()))?;

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

        // Validate record_globals names exist in globals.
        for rg in &port.memory.record_globals {
            if !port.memory.globals.contains_key(&rg.name) {
                return Err(format!("record_globals names unknown global '{}'", rg.name));
            }
        }

        // Validate hitstun_sources names appear in the recorded-globals union.
        if let Some(hs) = &port.hitstun_sources {
            let recorded_names: Vec<&str> = port.gate
                .iter()
                .filter_map(|c| c.global_name())
                .chain(port.memory.record_globals.iter().map(|rg| rg.name.as_str()))
                .collect();
            for global_name in hs.values() {
                if !recorded_names.iter().any(|n| *n == global_name) {
                    return Err(format!(
                        "hitstun_sources names unrecorded global '{}'",
                        global_name
                    ));
                }
            }
        }

        // Validate fighter fields: exactly one source, and globals resolve.
        for f in &port.memory.fighter_fields {
            match (&f.off, &f.globals) {
                (Some(_), Some(_)) => {
                    return Err(format!(
                        "fighter field '{}' has both off and globals — pick one",
                        f.name
                    ));
                }
                (None, None) => {
                    return Err(format!(
                        "fighter field '{}' needs off or globals",
                        f.name
                    ));
                }
                (None, Some(g)) => {
                    for gname in [&g.block1, &g.block2] {
                        if !port.memory.globals.contains_key(gname) {
                            return Err(format!(
                                "fighter field '{}' names unknown global '{gname}'",
                                f.name
                            ));
                        }
                    }
                }
                (Some(_), None) => {}
            }
        }

        // Validate pin globals resolve.
        for pin in &port.pins {
            if !port.memory.globals.contains_key(&pin.global) {
                return Err(format!("pins names unknown global '{}'", pin.global));
            }
        }

        // Validate id_map values exist in family roster.
        if let Some(im) = &port.id_map {
            for canonical_id in im.values() {
                if !family.roster.iter().any(|r| r.id == *canonical_id) {
                    return Err(format!("id_map maps to unknown roster id {}", canonical_id));
                }
            }
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

        // Macro-action vocabulary (MACRO_ACTIONS §1/§2): moves are keyed by
        // roster NAME; encodings must reference declared moves, chord-backed
        // classes, and dirs from the closed semantic set.
        for chr in family.moves.keys() {
            if !family.roster.iter().any(|r| r.name == *chr) {
                return Err(format!("moves names unknown roster character '{chr}'"));
            }
        }
        const DIRS: [&str; 4] = ["back", "forward", "up", "down"];
        for (chr, mvs) in &port.special_inputs {
            let Some(fam_moves) = family.moves.get(chr) else {
                return Err(format!("special_inputs names character '{chr}' absent from family moves"));
            };
            for (mv, steps) in mvs {
                if !fam_moves.iter().any(|m| m.name == *mv) {
                    return Err(format!("special_inputs '{chr}' encodes unknown move '{mv}'"));
                }
                if steps.is_empty() {
                    return Err(format!("special_inputs '{chr}.{mv}' has no steps"));
                }
                for st in steps {
                    if st.dirs.is_empty() && st.press.is_empty() {
                        return Err(format!("special_inputs '{chr}.{mv}' has an empty step"));
                    }
                    for d in &st.dirs {
                        if !DIRS.contains(&d.as_str()) {
                            return Err(format!("special_inputs '{chr}.{mv}' names unknown dir '{d}'"));
                        }
                    }
                    for class in &st.press {
                        if !port.attack_chords.contains_key(class) {
                            return Err(format!(
                                "special_inputs '{chr}.{mv}' presses unknown class '{class}'"
                            ));
                        }
                    }
                }
            }
        }
        if let Some(cs) = &port.contact_signal {
            match (&cs.field, &cs.global) {
                (Some(_), Some(_)) => {
                    return Err("contact_signal: pick field OR global, not both".to_string());
                }
                (None, None) => {
                    return Err("contact_signal needs field or global".to_string());
                }
                (Some(f), None) => {
                    if !port.memory.fighter_fields.iter().any(|x| &x.name == f) {
                        return Err(format!("contact_signal names unknown field '{f}'"));
                    }
                }
                (None, Some(gl)) => {
                    if !port.memory.globals.contains_key(gl) {
                        return Err(format!("contact_signal names unknown global '{gl}'"));
                    }
                }
            }
        }
        Ok(GameProfile { dir: fam_dir, family, port })
    }

    /// Resolve a game path to (family_dir, profile_path).
    ///
    /// Handles both directory and port-selector cases per the §5.2 contract:
    /// 1. `dir` is a directory → family dir = dir; profile = <dirname>.profile.json or single *.profile.json
    /// 2. `dir` is not a directory but parent is → family dir = parent; selector = basename;
    ///    try <parent>/<basename>.profile.json, then scan for matching "port" field
    /// 3. Neither → error
    fn resolve_game_dir(input: &Path) -> Result<(PathBuf, PathBuf), String> {
        if input.is_dir() {
            // Case 1: dir is a directory.
            let fam_dir = input.to_path_buf();
            let stem = fam_dir
                .file_name()
                .map(|s| s.to_string_lossy().into_owned());

            // Try <dirname>.profile.json first.
            let default_path = stem
                .as_deref()
                .map(|s| fam_dir.join(format!("{s}.profile.json")));
            if let Some(path) = default_path {
                if path.is_file() {
                    return Ok((fam_dir, path));
                }
            }

            // Try single *.profile.json.
            let profiles: Vec<PathBuf> = std::fs::read_dir(&fam_dir)
                .ok()
                .and_then(|entries| {
                    let found: Vec<PathBuf> = entries
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| p.to_string_lossy().ends_with(".profile.json"))
                        .collect();
                    if found.is_empty() { None } else { Some(found) }
                })
                .unwrap_or_default();

            if profiles.is_empty() {
                return Err(format!("{}: no *.profile.json found", fam_dir.display()));
            }
            if profiles.len() == 1 {
                return Ok((fam_dir, profiles[0].clone()));
            }

            // Multiple profiles, no default → error with suggestion.
            let stems: Vec<String> = profiles
                .iter()
                .filter_map(|p| {
                    // file_stem on "mk2.profile.json" is "mk2.profile" — trim
                    // the ".profile" so the suggestion names the port segment.
                    p.file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.trim_end_matches(".profile").to_string())
                })
                .collect();
            return Err(format!(
                "{}: multiple port profiles and no {}.profile.json default — select one: --game {}/{}",
                fam_dir.display(),
                stem.as_deref().unwrap_or(""),
                fam_dir.display(),
                stems.join("|")
            ));
        }

        // Case 2: not a directory; check if parent is a directory.
        if let Some(parent) = input.parent() {
            if parent.is_dir() {
                let fam_dir = parent.to_path_buf();
                let selector = input
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .ok_or_else(|| format!("--game {}: invalid path", input.display()))?;

                // Try <parent>/<selector>.profile.json.
                let default_path = fam_dir.join(format!("{}.profile.json", selector));
                if default_path.is_file() {
                    return Ok((fam_dir, default_path));
                }

                // Scan for matching "port" field.
                let profiles: Vec<(PathBuf, String)> = std::fs::read_dir(&fam_dir)
                    .ok()
                    .and_then(|entries| {
                        let mut matches = Vec::new();
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.to_string_lossy().ends_with(".profile.json") {
                                if let Ok(content) = std::fs::read_to_string(&path) {
                                    if let Ok(obj) =
                                        serde_json::from_str::<serde_json::Value>(&content)
                                    {
                                        if let Some(port_val) = obj.get("port") {
                                            if let Some(port_str) = port_val.as_str() {
                                                if port_str == selector {
                                                    matches.push((
                                                        path.clone(),
                                                        port_str.to_string(),
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if matches.is_empty() {
                            None
                        } else {
                            Some(matches)
                        }
                    })
                    .unwrap_or_default();

                if profiles.is_empty() {
                    // Collect available profiles for the error message.
                    let available: Vec<String> = std::fs::read_dir(&fam_dir)
                        .ok()
                        .and_then(|entries| {
                            let mut stems = Vec::new();
                            let mut ports = Vec::new();
                            for entry in entries.flatten() {
                                let path = entry.path();
                                if path.to_string_lossy().ends_with(".profile.json") {
                                    if let Some(stem) = path.file_stem() {
                                        stems.push(
                                            stem.to_string_lossy()
                                                .trim_end_matches(".profile")
                                                .to_string(),
                                        );
                                    }
                                    if let Ok(content) = std::fs::read_to_string(&path) {
                                        if let Ok(obj) = serde_json::from_str::<serde_json::Value>(
                                            &content,
                                        ) {
                                            if let Some(port_val) = obj.get("port") {
                                                if let Some(port_str) = port_val.as_str() {
                                                    ports.push(port_str.to_string());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            stems.sort();
                            ports.sort();
                            stems.extend(ports);
                            if stems.is_empty() {
                                None
                            } else {
                                Some(stems)
                            }
                        })
                        .unwrap_or_default();

                    let available_str = if available.is_empty() {
                        "none".to_string()
                    } else {
                        available.join("/")
                    };

                    return Err(format!(
                        "{}: no port '{}' (no {}.profile.json and no profile with \"port\": \"{}\"); available: {}",
                        fam_dir.display(), selector, selector, selector, available_str
                    ));
                }

                if profiles.len() == 1 {
                    return Ok((fam_dir, profiles[0].0.clone()));
                }

                // Ambiguous: multiple matches.
                let files: Vec<String> = profiles
                    .iter()
                    .map(|(p, _)| p.file_name().unwrap().to_string_lossy().to_string())
                    .collect();
                return Err(format!(
                    "{}: port '{}' is ambiguous: {}",
                    fam_dir.display(),
                    selector,
                    files.join(", ")
                ));
            }
        }

        // Case 3: neither conditions met.
        Err(format!("--game {}: no such game directory", input.display()))
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

    /// Offset + size for an OFFSET-based fighter field. Returns None for
    /// global-sourced fields (callers that need those use [`field_addr`],
    /// or fall back to the per-player globals themselves — training's
    /// x_pair pattern).
    pub fn field_off(&self, name: &str) -> Option<(u32, u8)> {
        self.port
            .memory
            .fighter_fields
            .iter()
            .find(|f| f.name == name)
            .and_then(|f| f.off.as_ref().map(|o| (o.0, f.size)))
    }

    /// ABSOLUTE address + size of a fighter field for one block (1 or 2),
    /// resolving both variants: block base + offset, or the per-block global.
    pub fn field_addr(&self, block: u8, name: &str) -> Option<(u32, u8)> {
        let f = self.port.memory.fighter_fields.iter().find(|f| f.name == name)?;
        let base = if block == 1 { self.block1() } else { self.block2() };
        if let Some(off) = &f.off {
            return Some((base.wrapping_add(off.0), f.size));
        }
        let g = f.globals.as_ref()?;
        let gname = if block == 1 { &g.block1 } else { &g.block2 };
        Some((self.global(gname)?, f.size))
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

    /// Session pins resolved to (address, value) pairs — profile load
    /// guarantees every pin global resolves.
    pub fn resolved_pins(&self) -> Vec<(u32, u8)> {
        self.port
            .pins
            .iter()
            .filter_map(|p| Some((self.global(&p.global)?, p.value)))
            .collect()
    }

    /// The family∩port special intersection for a CANONICAL char id — the
    /// legal offer list (panel, executor): family `moves` order, only the
    /// moves this port encodes. Empty when either side omits the character.
    pub fn specials_for(&self, canon_id: u8) -> Vec<(&str, &[StepSpec])> {
        let Some(name) = self.family.roster.iter().find(|r| r.id == canon_id).map(|r| &r.name)
        else {
            return Vec::new();
        };
        let (Some(fam), Some(enc)) =
            (self.family.moves.get(name), self.port.special_inputs.get(name))
        else {
            return Vec::new();
        };
        fam.iter()
            .filter_map(|m| enc.get(&m.name).map(|s| (m.name.as_str(), s.as_slice())))
            .collect()
    }

    /// Every encoding this port carries (family∩port across all characters),
    /// in deterministic (character, family-move) order — the recorder's
    /// char-blind matcher input. Duplicate move names across characters are
    /// deduped by the matcher at emission time, not here.
    pub fn all_specials(&self) -> Vec<(&str, &[StepSpec])> {
        self.port
            .special_inputs
            .iter()
            .filter_map(|(chr, enc)| self.family.moves.get(chr).map(|fam| (fam, enc)))
            .flat_map(|(fam, enc)| {
                fam.iter()
                    .filter_map(|m| enc.get(&m.name).map(|s| (m.name.as_str(), s.as_slice())))
            })
            .collect()
    }

    /// Translate a raw RAM char id to its canonical roster id.
    /// If no id_map is present or the raw id is not in the map, returns identity (raw).
    #[allow(dead_code)]
    pub fn canon_char_id(&self, raw: u8) -> u8 {
        self.port
            .id_map
            .as_ref()
            .and_then(|m| m.get(&raw.to_string()).copied())
            .unwrap_or(raw)
    }
}

/// RETRO joypad bit for a button name as used in `attack_chords` (the
/// RETRO_DEVICE_ID order every mask in the codebase shares).
pub fn retro_button_bit(name: &str) -> Option<u16> {
    Some(match name {
        "b" => 0,
        "y" => 1,
        "select" => 2,
        "start" => 3,
        "up" => 4,
        "down" => 5,
        "left" => 6,
        "right" => 7,
        "a" => 8,
        "x" => 9,
        "l" => 10,
        "r" => 11,
        _ => return None,
    })
}

impl GateCond {
    pub fn global_name(&self) -> Option<&str> {
        match self {
            GateCond::ByteZero { global }
            | GateCond::WordZero { global }
            | GateCond::BcdValidNonzero { global }
            | GateCond::WordMaskedNotAll { global, .. } => Some(global),
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
    use std::fs;

    /// Create a unique temp directory path for tests. The caller is responsible for cleanup.
    fn make_test_dir(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join("rustretro_tests");
        let _ = fs::create_dir_all(&base);
        let path = base.join(format!(
            "{}_{}_{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&path); // Clean up if it exists
        fs::create_dir_all(&path).ok();
        path
    }

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

    #[test]
    fn path_resolution_default_directory_with_matching_stem() {
        // Test case 1: dir exists, <dirname>.profile.json exists → use it
        let tmpbase = make_test_dir("path_resolution_default");
        let game_dir = tmpbase.join("mygame");
        fs::create_dir(&game_dir).unwrap();

        // Create family.json
        let family_json = r#"{"family":"test","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        // Create mygame.profile.json (default via stem)
        let port_json = r#"{"family":"test","port":"default","core":{"library_name":"","provenance_game":"test","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("mygame.profile.json"), port_json).unwrap();

        let result = GameProfile::load(&game_dir);
        assert!(result.is_ok());
        let profile = result.unwrap();
        assert_eq!(profile.family.family, "test");
        assert_eq!(profile.port.port, "default");
    }

    #[test]
    fn path_resolution_single_profile_fallback() {
        // Test case 1b: dir exists, no <dirname>.profile.json, single *.profile.json → use it
        let tmpbase = make_test_dir("path_resolution_single");
        let game_dir = tmpbase.join("gameX");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"test2","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        // Only one profile with a different name
        let port_json = r#"{"family":"test2","port":"only","core":{"library_name":"","provenance_game":"test2","provenance_core":"test2"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("other.profile.json"), port_json).unwrap();

        let result = GameProfile::load(&game_dir);
        assert!(result.is_ok());
        let profile = result.unwrap();
        assert_eq!(profile.port.port, "only");
    }

    #[test]
    fn path_resolution_multiple_profiles_error() {
        // Test case 1c: dir exists, multiple *.profile.json, no default → error
        let tmpbase = make_test_dir("path_resolution_multiple");
        let game_dir = tmpbase.join("ambiguous");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"test3","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json_a = r#"{"family":"test3","port":"arcade","core":{"library_name":"","provenance_game":"test3","provenance_core":"test3"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("arcade.profile.json"), port_json_a).unwrap();

        let port_json_g = r#"{"family":"test3","port":"genesis","core":{"library_name":"","provenance_game":"test3","provenance_core":"test3"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("genesis.profile.json"), port_json_g).unwrap();

        let result = GameProfile::load(&game_dir);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("multiple port profiles"));
        assert!(err.contains("arcade") || err.contains("genesis"));
    }

    #[test]
    fn path_resolution_port_segment_by_filename() {
        // Test case 2a: dir/selector path where selector.profile.json exists
        let tmpbase = make_test_dir("path_resolution_port_segment");
        let game_dir = tmpbase.join("mk2");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"mk2","roster":[{"id":0,"name":"test"}],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json_mk2 = r#"{"family":"mk2","port":"arcade","core":{"library_name":"","provenance_game":"mk2","provenance_core":"fbneo"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":161,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("mk2.profile.json"), port_json_mk2).unwrap();

        let port_json_gen = r#"{"family":"mk2","port":"genesis","core":{"library_name":"","provenance_game":"mk2","provenance_core":"genesis_plus"},"memory":{"blocks":{"block1":"0xFF8000","block2":"0xFF8200","stride":"0x200"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":161,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("genesis.profile.json"), port_json_gen)
            .unwrap();

        // Load via selector path
        let selector_path = tmpbase.join("mk2/genesis");
        let result = GameProfile::load(&selector_path);
        assert!(result.is_ok());
        let profile = result.unwrap();
        assert_eq!(profile.port.port, "genesis");
    }

    #[test]
    fn path_resolution_port_segment_by_field_match() {
        // Test case 2b: dir/selector where selector matches a "port" field value
        let tmpbase = make_test_dir("path_resolution_port_field");
        let game_dir = tmpbase.join("mk2");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"mk2","roster":[{"id":0,"name":"test"}],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        // File named differently but port="arcade"
        let port_json_default = r#"{"family":"mk2","port":"arcade","core":{"library_name":"","provenance_game":"mk2","provenance_core":"fbneo"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":161,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("mk2.profile.json"), port_json_default).unwrap();

        // File named port_v2 but port="v2"
        let port_json_v2 = r#"{"family":"mk2","port":"v2","core":{"library_name":"","provenance_game":"mk2","provenance_core":"fbneo"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":161,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("port_v2.profile.json"), port_json_v2).unwrap();

        // Try to load via --game mk2/v2 (should match the port field, not filename)
        let selector_path = tmpbase.join("mk2/v2");
        let result = GameProfile::load(&selector_path);
        assert!(result.is_ok());
        let profile = result.unwrap();
        assert_eq!(profile.port.port, "v2");
    }

    #[test]
    fn path_resolution_port_segment_ambiguous_error() {
        // Test case 2d: multiple profiles with the same port field value → error
        let tmpbase = make_test_dir("path_resolution_ambiguous");
        let game_dir = tmpbase.join("bad");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"bad","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json_1 = r#"{"family":"bad","port":"dup","core":{"library_name":"","provenance_game":"bad","provenance_core":"bad"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("first.profile.json"), port_json_1).unwrap();

        let port_json_2 = r#"{"family":"bad","port":"dup","core":{"library_name":"","provenance_game":"bad","provenance_core":"bad"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("second.profile.json"), port_json_2).unwrap();

        let selector_path = tmpbase.join("bad/dup");
        let result = GameProfile::load(&selector_path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("ambiguous"));
    }

    #[test]
    fn path_resolution_port_segment_not_found_error() {
        // Test case 2c: selector doesn't match any profile → error
        let tmpbase = make_test_dir("path_resolution_not_found");
        let game_dir = tmpbase.join("mk2");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"mk2","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"mk2","port":"arcade","core":{"library_name":"","provenance_game":"mk2","provenance_core":"fbneo"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":161,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("mk2.profile.json"), port_json).unwrap();

        let selector_path = tmpbase.join("mk2/nonexistent");
        let result = GameProfile::load(&selector_path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("no port 'nonexistent'"));
    }

    #[test]
    fn id_map_present_and_mapped() {
        // canon_char_id with a present id_map entry
        let tmpbase = make_test_dir("id_map_present");
        let game_dir = tmpbase.join("mapped");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"mapped","roster":[{"id":0,"name":"a"},{"id":1,"name":"b"},{"id":2,"name":"c"}],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"mapped","port":"test","core":{"library_name":"","provenance_game":"mapped","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{},"id_map":{"5":0,"6":1,"7":2}}"#;
        fs::write(game_dir.join("mapped.profile.json"), port_json).unwrap();

        let profile = GameProfile::load(&game_dir).unwrap();
        assert_eq!(profile.canon_char_id(5), 0);
        assert_eq!(profile.canon_char_id(6), 1);
        assert_eq!(profile.canon_char_id(7), 2);
    }

    #[test]
    fn id_map_absent_uses_identity() {
        // canon_char_id with no id_map → identity
        let tmpbase = make_test_dir("id_map_absent");
        let game_dir = tmpbase.join("nomapped");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"nomapped","roster":[{"id":0,"name":"a"},{"id":5,"name":"b"}],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"nomapped","port":"test","core":{"library_name":"","provenance_game":"nomapped","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("nomapped.profile.json"), port_json).unwrap();

        let profile = GameProfile::load(&game_dir).unwrap();
        assert_eq!(profile.canon_char_id(5), 5);
        assert_eq!(profile.canon_char_id(0), 0);
    }

    #[test]
    fn id_map_unmapped_key_uses_identity() {
        // canon_char_id with id_map present but key missing → identity
        let tmpbase = make_test_dir("id_map_unmapped");
        let game_dir = tmpbase.join("partial");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"partial","roster":[{"id":0,"name":"a"},{"id":1,"name":"b"},{"id":5,"name":"c"}],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"partial","port":"test","core":{"library_name":"","provenance_game":"partial","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{},"id_map":{"5":0}}"#;
        fs::write(game_dir.join("partial.profile.json"), port_json).unwrap();

        let profile = GameProfile::load(&game_dir).unwrap();
        assert_eq!(profile.canon_char_id(5), 0); // mapped
        assert_eq!(profile.canon_char_id(99), 99); // unmapped → identity
    }

    #[test]
    fn record_globals_valid() {
        // record_globals with valid globals
        let tmpbase = make_test_dir("record_globals_valid");
        let game_dir = tmpbase.join("recorded");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"recorded","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"recorded","port":"test","core":{"library_name":"","provenance_game":"recorded","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{"combo":"0x1000","demo":"0x2000"},"record_globals":[{"name":"combo","size":1},{"name":"demo","size":2}]},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("recorded.profile.json"), port_json).unwrap();

        let result = GameProfile::load(&game_dir);
        assert!(result.is_ok());
    }

    #[test]
    fn record_globals_invalid_global_name() {
        // record_globals with an unknown global → error
        let tmpbase = make_test_dir("record_globals_invalid");
        let game_dir = tmpbase.join("badrecord");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"badrecord","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"badrecord","port":"test","core":{"library_name":"","provenance_game":"badrecord","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{"known":"0x1000"},"record_globals":[{"name":"unknown","size":1}]},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{}}"#;
        fs::write(game_dir.join("badrecord.profile.json"), port_json).unwrap();

        let result = GameProfile::load(&game_dir);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("record_globals names unknown global"));
    }

    #[test]
    fn hitstun_sources_valid() {
        // hitstun_sources with valid recorded globals
        let tmpbase = make_test_dir("hitstun_sources_valid");
        let game_dir = tmpbase.join("hitstun");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"hitstun","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"hitstun","port":"test","core":{"library_name":"","provenance_game":"hitstun","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{"combo_b1":"0x1000","combo_b2":"0x2000"},"record_globals":[{"name":"combo_b1","size":1},{"name":"combo_b2","size":1}]},"gate":[{"kind":"byte_zero","global":"combo_b1"}],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{},"hitstun_sources":{"block1":"combo_b1","block2":"combo_b2"}}"#;
        fs::write(game_dir.join("hitstun.profile.json"), port_json).unwrap();

        let result = GameProfile::load(&game_dir);
        assert!(result.is_ok());
    }

    #[test]
    fn hitstun_sources_unrecorded_global() {
        // hitstun_sources references a global not in the recorded union → error
        let tmpbase = make_test_dir("hitstun_sources_unrecorded");
        let game_dir = tmpbase.join("badhitstun");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"badhitstun","roster":[],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"badhitstun","port":"test","core":{"library_name":"","provenance_game":"badhitstun","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{"combo_b1":"0x1000","unrecorded":"0x3000"}},"gate":[{"kind":"byte_zero","global":"combo_b1"}],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{},"hitstun_sources":{"block1":"unrecorded"}}"#;
        fs::write(game_dir.join("badhitstun.profile.json"), port_json).unwrap();

        let result = GameProfile::load(&game_dir);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("hitstun_sources names unrecorded global"));
    }

    #[test]
    fn mk2_ships_the_reptile_special_intersection() {
        let p = GameProfile::load(Path::new("library/mk2")).expect("mk2 profile loads");
        // Family order (moves list), only what the port encodes.
        let names: Vec<&str> = p.specials_for(9).iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["slide", "acid_spit", "force_ball"]);
        // slide: single chord step, back+LK+LP+Block (the §2 two-button
        // chord was live-DISPROVEN — the game performs a normal without
        // Block; slide pose + hit verified with Block in the chord).
        let (_, slide) = p.specials_for(9)[0];
        assert_eq!(slide.len(), 1);
        assert_eq!(slide[0].dirs, vec!["back"]);
        assert_eq!(slide[0].press, vec!["LK", "LP", "Block"]);
        assert_eq!(slide[0].frames, 8);
        // Characters without encodings offer nothing.
        assert!(p.specials_for(1).is_empty(), "liukang has no specials this phase");
        assert!(p.specials_for(99).is_empty());
        assert_eq!(p.all_specials().len(), 3);
        // The arcade contact signal is the per-fighter `action_counter`
        // FIELD (quiet while guarding, fires on blocked contact even at zero
        // chip — live-verified). hitstun_sources stays as the health-delta
        // fallback and as the hitstun FEATURE source.
        assert!(p.port.contact_signal.is_none(), "mk2 uses the hitstun_sources fallback");
        let hs = p.port.hitstun_sources.as_ref().unwrap();
        assert_eq!(hs.get("block1").map(String::as_str), Some("p1_health_hud"));
        assert_eq!(hs.get("block2").map(String::as_str), Some("p2_health_hud"));
        // asurabld stays macro-free — that is the back-compat gate.
        let a = init_for_tests();
        assert!(a.family.moves.is_empty());
        assert!(a.all_specials().is_empty());
        assert!(a.port.contact_signal.is_none());
    }

    /// Build a family+port pair with macro fields spliced in, and load it.
    fn load_with_macros(tag: &str, moves: &str, port_extra: &str) -> Result<GameProfile, String> {
        let dir = make_test_dir(tag).join("g");
        fs::create_dir(&dir).unwrap();
        let family_json = format!(
            r#"{{"family":"g","roster":[{{"id":9,"name":"reptile"}}],"move_classes":[],
                "attack_classes":["None","LK","LP"],"moves":{moves}}}"#
        );
        fs::write(dir.join("family.json"), family_json).unwrap();
        let port_json = format!(
            r#"{{"family":"g","port":"test","core":{{"library_name":"","provenance_game":"g","provenance_core":"g"}},
                "memory":{{"blocks":{{"block1":"0x0","block2":"0x0","stride":"0x0"}},"fighter_fields":[],"globals":{{"hits":"0x100"}}}},
                "gate":[],"enforcement":{{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0}},
                "calibration":{{}},"attack_chords":{{"LK":["a"],"LP":["b"]}}{port_extra}}}"#
        );
        fs::write(dir.join("g.profile.json"), port_json).unwrap();
        GameProfile::load(&dir)
    }

    #[test]
    fn macro_schema_validation_rejects_bad_references() {
        let moves = r#"{"reptile":[{"name":"slide","tags":["special"]}]}"#;
        // Valid baseline parses.
        assert!(load_with_macros(
            "macros_ok", moves,
            r#","special_inputs":{"reptile":{"slide":[{"dirs":["back"],"press":["LK","LP"]}]}},"contact_signal":{"global":"hits"}"#
        ).is_ok());
        // moves keyed by a name not in the roster.
        assert!(load_with_macros("macros_badchar", r#"{"ghost":[{"name":"boo"}]}"#, "")
            .unwrap_err()
            .contains("unknown roster character"));
        // special_inputs for a character with no family moves.
        assert!(load_with_macros(
            "macros_nochar", "{}",
            r#","special_inputs":{"reptile":{"slide":[{"press":["LK"]}]}}"#
        ).unwrap_err().contains("absent from family moves"));
        // Encoding an undeclared move.
        assert!(load_with_macros(
            "macros_badmove", moves,
            r#","special_inputs":{"reptile":{"warp":[{"press":["LK"]}]}}"#
        ).unwrap_err().contains("unknown move 'warp'"));
        // Dir outside the closed semantic set.
        assert!(load_with_macros(
            "macros_baddir", moves,
            r#","special_inputs":{"reptile":{"slide":[{"dirs":["left"],"press":["LK"]}]}}"#
        ).unwrap_err().contains("unknown dir 'left'"));
        // Press class with no chord entry.
        assert!(load_with_macros(
            "macros_badclass", moves,
            r#","special_inputs":{"reptile":{"slide":[{"press":["HP"]}]}}"#
        ).unwrap_err().contains("unknown class 'HP'"));
        // Empty step / empty step list.
        assert!(load_with_macros(
            "macros_emptystep", moves,
            r#","special_inputs":{"reptile":{"slide":[{}]}}"#
        ).unwrap_err().contains("empty step"));
        assert!(load_with_macros(
            "macros_nosteps", moves,
            r#","special_inputs":{"reptile":{"slide":[]}}"#
        ).unwrap_err().contains("no steps"));
        // contact_signal must name a mapped global.
        assert!(load_with_macros("macros_badsignal", moves, r#","contact_signal":{"global":"nope"}"#)
            .unwrap_err()
            .contains("contact_signal names unknown global"));
    }

    #[test]
    fn id_map_invalid_roster_id() {
        // id_map references a non-existent roster id → error
        let tmpbase = make_test_dir("id_map_invalid_roster");
        let game_dir = tmpbase.join("badidmap");
        fs::create_dir(&game_dir).unwrap();

        let family_json = r#"{"family":"badidmap","roster":[{"id":0,"name":"a"},{"id":1,"name":"b"}],"move_classes":[],"attack_classes":[]}"#;
        fs::write(game_dir.join("family.json"), family_json).unwrap();

        let port_json = r#"{"family":"badidmap","port":"test","core":{"library_name":"","provenance_game":"badidmap","provenance_core":"test"},"memory":{"blocks":{"block1":"0x0","block2":"0x0","stride":"0x0"},"fighter_fields":[],"globals":{}},"gate":[],"enforcement":{"health_max":255,"refill_below":1,"timer_hold":[0,0],"credits_target":0,"credits_min":0},"calibration":{},"attack_chords":{},"id_map":{"5":99}}"#;
        fs::write(game_dir.join("badidmap.profile.json"), port_json).unwrap();

        let result = GameProfile::load(&game_dir);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("id_map maps to unknown roster id"));
    }
}
