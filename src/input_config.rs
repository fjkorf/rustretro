//! Configurable input mappings (keyboard + gamepad → RETRO joypad bits).
//!
//! Loaded once at startup into a Bevy `Resource`; `read_input` builds each
//! port's `[bool; 12]` from (keyboard bindings ∪ gamepad bindings ∪ MCP
//! injection). Resolution order: `--keymap PATH` (parse errors are fatal) →
//! `<save_dir>/<rom_stem>.keymap.json` → `<save_dir>/keymap.json` → built-in
//! default (identical to the historical hardcoded maps). `--dump-keymap`
//! prints the active config; `--calibrate` writes one interactively.
//!
//! Gamepad values are LISTS of RETRO buttons so one physical button can emit
//! a chord (Asura Blade: weapon toss = B+A+Y, launcher = B+A). The left stick
//! is structural, not per-button config: it always drives the four direction
//! bits through `stick_deadzone` (hat/d-pad lever modes bind `DPad*` buttons
//! in the map instead). Key/button names are Bevy's enum variant names —
//! exactly what `--pad-debug` prints (e.g. "KeyZ", "South", "RightTrigger2").
//! Limitation: `GamepadButton::Other(n)` / non-unit `KeyCode`s cannot be JSON
//! map keys; no current device needs them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::input::gamepad::GamepadButton;
use bevy::input::keyboard::KeyCode;
use bevy::math::Vec2;
use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};

/// RETRO_DEVICE_ID_JOYPAD order — discriminant = bit index.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum RetroButton {
    B,
    Y,
    Select,
    Start,
    Up,
    Down,
    Left,
    Right,
    A,
    X,
    L,
    R,
}

impl RetroButton {
    pub fn idx(self) -> usize {
        self as usize
    }

    /// Parse the lowercase RETRO name used by profile `attack_chords` and
    /// the MCP `press_buttons` tool ("b", "a", "start", ...).
    pub fn from_retro_name(name: &str) -> Option<RetroButton> {
        use RetroButton::*;
        Some(match name {
            "b" => B,
            "y" => Y,
            "select" => Select,
            "start" => Start,
            "up" => Up,
            "down" => Down,
            "left" => Left,
            "right" => Right,
            "a" => A,
            "x" => X,
            "l" => L,
            "r" => R,
            _ => return None,
        })
    }
}

/// One-or-more RETRO bits emitted by a single physical control. Serializes
/// as a list; deserializes from a list OR a bare button name, so keymap v1
/// files (whose keyboard values were single buttons) load unchanged.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Chord(pub Vec<RetroButton>);

impl<'de> Deserialize<'de> for Chord {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum OneOrMany {
            One(RetroButton),
            Many(Vec<RetroButton>),
        }
        Ok(match OneOrMany::deserialize(d)? {
            OneOrMany::One(b) => Chord(vec![b]),
            OneOrMany::Many(v) => Chord(v),
        })
    }
}

impl From<RetroButton> for Chord {
    fn from(b: RetroButton) -> Self {
        Chord(vec![b])
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct PortMap {
    /// Key → RETRO bits (chords allowed; several keys may share a bit).
    pub keyboard: BTreeMap<KeyCode, Chord>,
    /// Generic gamepad map: one physical button → one-or-more RETRO bits.
    pub gamepad: BTreeMap<GamepadButton, Chord>,
    /// Device-specific overrides keyed by the pad's reported NAME (e.g.
    /// "Mayflash Arcade Fightstick F300"): a whole map replaces `gamepad`
    /// for that device. Lets a fightstick and a normal pad coexist with
    /// different layouts. Absent devices fall back to `gamepad`.
    pub gamepad_by_device: BTreeMap<String, BTreeMap<GamepadButton, Chord>>,
}

#[derive(Serialize, Deserialize, Clone, Resource)]
#[serde(default)]
pub struct InputConfig {
    /// Analog→digital threshold for the left stick (libretro convention;
    /// deliberately high for fighting games — no drift diagonals).
    pub stick_deadzone: f32,
    /// Sorted-pad-slot → port. `[1, 0]` swaps two connected pads.
    pub pad_order: Vec<usize>,
    /// Per-port maps, index = port (0 = P1, 1 = P2).
    pub ports: Vec<PortMap>,
}

impl Default for InputConfig {
    /// Reproduces the historical hardcoded maps exactly: P1/P2 keyboard rows
    /// and the Mayflash F300 PS3/DInput layout (top row L/M/H = South/West/
    /// North → B/A/Y, toss chord on RightTrigger, launcher chord on Mode,
    /// coin on LeftTrigger, Start on RightTrigger2 + LeftTrigger2).
    fn default() -> Self {
        use GamepadButton as G;
        use KeyCode as K;
        use RetroButton::*;
        let f300: BTreeMap<G, Chord> = [
            (G::South, Chord(vec![B])),
            (G::West, Chord(vec![A])),
            (G::North, Chord(vec![Y])),
            (G::East, Chord(vec![X])),
            (G::RightTrigger, Chord(vec![B, A, Y])), // weapon toss chord
            (G::Mode, Chord(vec![B, A])),            // launcher chord
            (G::LeftTrigger, Chord(vec![Select])),
            (G::RightTrigger2, Chord(vec![Start])),
            (G::LeftTrigger2, Chord(vec![Start])),
            (G::DPadUp, Chord(vec![Up])),
            (G::DPadDown, Chord(vec![Down])),
            (G::DPadLeft, Chord(vec![Left])),
            (G::DPadRight, Chord(vec![Right])),
        ]
        .into_iter()
        .collect();
        let p1_keys: BTreeMap<K, Chord> = [
            (K::KeyZ, B),
            (K::KeyA, Y),
            (K::ShiftLeft, Select),
            (K::ShiftRight, Select),
            (K::Enter, Start),
            (K::ArrowUp, Up),
            (K::ArrowDown, Down),
            (K::ArrowLeft, Left),
            (K::ArrowRight, Right),
            (K::KeyX, A),
            (K::KeyS, X),
            (K::KeyQ, L),
            (K::KeyW, R),
        ]
        .into_iter()
        .map(|(k, b)| (k, Chord::from(b)))
        .collect();
        let p2_keys: BTreeMap<K, Chord> = [
            (K::KeyG, B),
            (K::KeyH, Y),
            (K::KeyN, Select),
            (K::KeyM, Start),
            (K::KeyI, Up),
            (K::KeyK, Down),
            (K::KeyJ, Left),
            (K::KeyL, Right),
            (K::KeyT, A),
            (K::KeyY, X),
            (K::KeyU, L),
            (K::KeyO, R),
        ]
        .into_iter()
        .map(|(k, b)| (k, Chord::from(b)))
        .collect();
        InputConfig {
            stick_deadzone: 0.5,
            pad_order: vec![0, 1],
            ports: vec![
                PortMap { keyboard: p1_keys, gamepad: f300.clone(), gamepad_by_device: BTreeMap::new() },
                PortMap { keyboard: p2_keys, gamepad: f300, gamepad_by_device: BTreeMap::new() },
            ],
        }
    }
}

impl InputConfig {
    /// Resolve + load: explicit flag (parse errors fatal) → per-ROM sidecar →
    /// global `keymap.json` → built-in default. Logs the chosen source.
    pub fn load(flag: &Option<PathBuf>, save_dir: &Path, rom: &str) -> InputConfig {
        if let Some(p) = flag {
            let text = std::fs::read_to_string(p)
                .unwrap_or_else(|e| panic!("--keymap {}: {e}", p.display()));
            let cfg = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("--keymap {}: parse error: {e}", p.display()));
            eprintln!("[keymap] loaded {}", p.display());
            return cfg;
        }
        let rom_stem = Path::new(rom)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        for cand in [
            save_dir.join(format!("{rom_stem}.keymap.json")),
            save_dir.join("keymap.json"),
        ] {
            match std::fs::read_to_string(&cand) {
                Err(_) => continue,
                Ok(text) => match serde_json::from_str(&text) {
                    Ok(cfg) => {
                        eprintln!("[keymap] loaded {}", cand.display());
                        return cfg;
                    }
                    Err(e) => {
                        eprintln!(
                            "[keymap] failed to parse {} ({e}) — using defaults",
                            cand.display()
                        );
                        return InputConfig::default();
                    }
                },
            }
        }
        eprintln!("[keymap] no keymap file — using built-in defaults");
        InputConfig::default()
    }

    /// The path `--calibrate` writes: the global default location.
    pub fn global_path(save_dir: &Path) -> PathBuf {
        save_dir.join("keymap.json")
    }
}

/// Gamepad state → RETRO bits for one port. Takes a pressed-closure instead of
/// `&Gamepad` so tests need no ECS.
pub fn pad_bits(
    pressed: impl Fn(GamepadButton) -> bool,
    stick: Vec2,
    port: &PortMap,
    deadzone: f32,
) -> [bool; 12] {
    pad_bits_for_device(pressed, stick, port, deadzone, None)
}

/// Like [`pad_bits`], selecting a device-specific map by the pad's reported
/// name when one exists (`gamepad_by_device`), else the generic `gamepad`.
pub fn pad_bits_for_device(
    pressed: impl Fn(GamepadButton) -> bool,
    stick: Vec2,
    port: &PortMap,
    deadzone: f32,
    device_name: Option<&str>,
) -> [bool; 12] {
    let map = device_name
        .and_then(|n| port.gamepad_by_device.get(n))
        .unwrap_or(&port.gamepad);
    let mut bits = [false; 12];
    for (btn, chord) in map {
        if pressed(*btn) {
            for id in &chord.0 {
                bits[id.idx()] = true;
            }
        }
    }
    bits[RetroButton::Up.idx()] |= stick.y > deadzone;
    bits[RetroButton::Down.idx()] |= stick.y < -deadzone;
    bits[RetroButton::Left.idx()] |= stick.x < -deadzone;
    bits[RetroButton::Right.idx()] |= stick.x > deadzone;
    bits
}

/// Keyboard state → RETRO bits for one port.
pub fn key_bits(pressed: impl Fn(KeyCode) -> bool, port: &PortMap) -> [bool; 12] {
    let mut bits = [false; 12];
    for (key, chord) in &port.keyboard {
        if pressed(*key) {
            for id in &chord.0 {
                bits[id.idx()] = true;
            }
        }
    }
    bits
}

/// Shorten a Bevy KeyCode debug name for display ("KeyZ" → "Z", arrows → glyphs).
pub fn key_name(k: KeyCode) -> String {
    let s = format!("{k:?}");
    match s.as_str() {
        "ArrowUp" => "↑".into(),
        "ArrowDown" => "↓".into(),
        "ArrowLeft" => "←".into(),
        "ArrowRight" => "→".into(),
        _ => s
            .strip_prefix("Key")
            .or_else(|| s.strip_prefix("Digit"))
            .map(str::to_string)
            .unwrap_or(s),
    }
}

/// Sorted RETRO bit-index set for a chord, so two chords can be compared as
/// sets regardless of the order their bits were listed/inserted in.
fn bit_set(bits: &[RetroButton]) -> Vec<usize> {
    let mut v: Vec<usize> = bits.iter().map(|b| b.idx()).collect();
    v.sort_unstable();
    v
}

/// Human-readable ACTION-oriented lines describing the ACTIVE mapping
/// (whatever `InputConfig::load` resolved — flag, sidecar, or default), for
/// the Help panel's "Game controls" section. One `(label, mapping)` pair per
/// action per port:
///   label   = "P{port} {action} ({RETRO bits joined by '+'})"
///   mapping = "{pad bindings} — {key bindings}", each side reverse-looked-up
///             against `cfg` by exact chord-bit-set match, or "no pad" /
///             "no key" when nothing binds that exact chord.
///
/// Example (asurabld, default keymap): action "Toss" = B+A+Y is bound only
/// on the F300's RightTrigger, no key reproduces the 3-button chord:
///   ("P1 Toss (B+A+Y)", "RightTrigger [pad] — no key")
///
/// LIMITATION: this is called once at startup (`main.rs`:
/// `ds.keymap_lines = input_config::summary(&keymap_cfg)`), before the core
/// has sent `RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS` — so the action names
/// come from `action_rows` run with *no* descriptors (profile + raw RETRO
/// names only; a core-only extra button the profile doesn't model won't
/// appear here). The Controls panel (owned separately) is the live view
/// that also sees descriptor names.
pub fn summary(cfg: &InputConfig) -> Vec<(String, String)> {
    let no_desc: [[Option<String>; 12]; 2] = Default::default();
    let mut out = Vec::new();
    for (port_idx, port) in cfg.ports.iter().enumerate().take(2) {
        let pn = port_idx + 1;
        for row in action_rows(port_idx, &no_desc) {
            let wanted = bit_set(&row.bits);
            let keys: Vec<String> = port
                .keyboard
                .iter()
                .filter(|(_, c)| bit_set(&c.0) == wanted)
                .map(|(k, _)| format!("{} [key]", key_name(*k)))
                .collect();
            let pads: Vec<String> = port
                .gamepad
                .iter()
                .filter(|(_, c)| bit_set(&c.0) == wanted)
                .map(|(b, _)| format!("{b:?} [pad]"))
                .collect();
            let pad_part = if pads.is_empty() { "no pad".to_string() } else { pads.join("/") };
            let key_part = if keys.is_empty() { "no key".to_string() } else { keys.join("/") };
            let bits_str: Vec<String> = row.bits.iter().map(|b| format!("{b:?}")).collect();
            let label = format!("P{pn} {} ({})", row.name, bits_str.join("+"));
            out.push((label, format!("{pad_part} — {key_part}")));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Vec2;

    fn only(set: &[GamepadButton]) -> impl Fn(GamepadButton) -> bool + '_ {
        move |b| set.contains(&b)
    }

    #[test]
    fn retro_button_indices_match_retro_ids() {
        use RetroButton::*;
        assert_eq!(
            [B, Y, Select, Start, Up, Down, Left, Right, A, X, L, R].map(|b| b.idx()),
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]
        );
    }

    #[test]
    fn golden_f300_layout() {
        use GamepadButton as G;
        let cfg = InputConfig::default();
        let port = &cfg.ports[0];
        let dz = cfg.stick_deadzone;
        let z = Vec2::ZERO;
        // singles
        let b = pad_bits(only(&[G::South]), z, port, dz);
        assert_eq!(b.iter().filter(|x| **x).count(), 1);
        assert!(b[0]); // B (Light)
        assert!(pad_bits(only(&[G::West]), z, port, dz)[8]); // A (Medium)
        assert!(pad_bits(only(&[G::North]), z, port, dz)[1]); // Y (Heavy)
        assert!(pad_bits(only(&[G::LeftTrigger]), z, port, dz)[2]); // coin
        assert!(pad_bits(only(&[G::RightTrigger2]), z, port, dz)[3]); // start
        assert!(pad_bits(only(&[G::LeftTrigger2]), z, port, dz)[3]); // start btn
        // chords
        let toss = pad_bits(only(&[G::RightTrigger]), z, port, dz);
        assert!(toss[0] && toss[8] && toss[1]);
        assert_eq!(toss.iter().filter(|x| **x).count(), 3);
        let launcher = pad_bits(only(&[G::Mode]), z, port, dz);
        assert!(launcher[0] && launcher[8] && !launcher[1]);
        // stick vs deadzone
        assert!(pad_bits(|_| false, Vec2::new(0.6, 0.0), port, dz)[7]);
        assert!(!pad_bits(|_| false, Vec2::new(0.4, 0.0), port, dz)[7]);
        assert!(pad_bits(|_| false, Vec2::new(0.0, 0.6), port, dz)[4]); // up
        // dpad
        assert!(pad_bits(only(&[G::DPadLeft]), z, port, dz)[6]);
    }

    #[test]
    fn golden_keyboard_maps() {
        use KeyCode as K;
        let cfg = InputConfig::default();
        let p1 = key_bits(|k| k == K::KeyZ, &cfg.ports[0]);
        assert!(p1[0] && p1.iter().filter(|x| **x).count() == 1);
        assert!(key_bits(|k| k == K::ShiftLeft, &cfg.ports[0])[2]);
        assert!(key_bits(|k| k == K::KeyM, &cfg.ports[1])[3]); // P2 start
        assert!(key_bits(|k| k == K::KeyJ, &cfg.ports[1])[6]); // P2 left
    }

    #[test]
    fn round_trip_and_partial_parse() {
        let cfg = InputConfig::default();
        let s = serde_json::to_string_pretty(&cfg).unwrap();
        let back: InputConfig = serde_json::from_str(&s).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), serde_json::to_string(&cfg).unwrap());
        // partial file: everything else defaulted
        let partial: InputConfig = serde_json::from_str(r#"{"stick_deadzone": 0.3}"#).unwrap();
        assert_eq!(partial.stick_deadzone, 0.3);
        assert_eq!(partial.ports.len(), 2);
        // bad key name errors
        assert!(serde_json::from_str::<InputConfig>(
            r#"{"ports":[{"keyboard":{"KeyZZ":"B"},"gamepad":{}}]}"#
        )
        .is_err());
    }

    #[test]
    fn keymap_v1_single_button_values_still_parse() {
        // v1 files wrote keyboard values as bare buttons; v2 is chord lists.
        let cfg: InputConfig = serde_json::from_str(
            r#"{"ports":[{"keyboard":{"KeyZ":"B","KeyX":["A","Y"]},"gamepad":{"South":["B"]}}]}"#,
        )
        .unwrap();
        let p = &cfg.ports[0];
        assert_eq!(p.keyboard[&KeyCode::KeyZ].0, vec![RetroButton::B]);
        // and v2 keyboard CHORDS work end-to-end:
        let bits = key_bits(|k| k == KeyCode::KeyX, p);
        assert!(bits[8] && bits[1] && !bits[0]);
    }

    #[test]
    fn device_specific_gamepad_map_overrides_generic() {
        use GamepadButton as G;
        let mut cfg = InputConfig::default();
        let mut stick_map: BTreeMap<G, Chord> = BTreeMap::new();
        stick_map.insert(G::South, Chord(vec![RetroButton::Y])); // swapped on the stick
        cfg.ports[0]
            .gamepad_by_device
            .insert("TestStick".into(), stick_map);
        let z = Vec2::ZERO;
        let only_south = |b: G| b == G::South;
        // Generic map: South = B.
        let generic = pad_bits_for_device(only_south, z, &cfg.ports[0], 0.5, None);
        assert!(generic[0] && !generic[1]);
        // Named device: South = Y.
        let dev = pad_bits_for_device(only_south, z, &cfg.ports[0], 0.5, Some("TestStick"));
        assert!(dev[1] && !dev[0]);
        // Unknown device name falls back to generic.
        let unk = pad_bits_for_device(only_south, z, &cfg.ports[0], 0.5, Some("Nope"));
        assert!(unk[0]);
    }

    #[test]
    fn action_rows_for_asurabld_match_the_profile_vocabulary() {
        crate::profile::init_for_tests();
        let no_desc: [[Option<String>; 12]; 2] = Default::default();
        let rows = action_rows(0, &no_desc);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            ["Up", "Down", "Left", "Right", "Start", "Coin",
             "Light", "Medium", "Heavy", "Launcher", "Toss"],
        );
        let toss = rows.iter().find(|r| r.name == "Toss").unwrap();
        assert_eq!(toss.bits, vec![RetroButton::B, RetroButton::A, RetroButton::Y]);
        assert_eq!(toss.source, ActionSource::Profile);
        // With a core descriptor on an unprofiled button, it appears.
        let mut desc: [[Option<String>; 12]; 2] = Default::default();
        desc[0][RetroButton::X.idx()] = Some("Taunt".into());
        let rows2 = action_rows(0, &desc);
        let taunt = rows2.iter().find(|r| r.name == "Taunt").unwrap();
        assert_eq!(taunt.bits, vec![RetroButton::X]);
        assert_eq!(taunt.source, ActionSource::Descriptor);
    }

    #[test]
    fn summary_is_action_oriented_with_reverse_lookup() {
        crate::profile::init_for_tests();
        let cfg = InputConfig::default();
        let lines = summary(&cfg);
        // One line per action per port; P1 has 11 actions (see the
        // action_rows test above), so P1 alone contributes 11 lines.
        let p1_lines: Vec<&(String, String)> =
            lines.iter().filter(|(l, _)| l.starts_with("P1 ")).collect();
        assert_eq!(p1_lines.len(), 11);

        // Toss = B+A+Y is bound only to the F300's RightTrigger chord; no
        // single key reproduces a 3-button chord.
        let (label, mapping) = lines.iter().find(|(l, _)| l.contains("Toss")).unwrap();
        assert_eq!(label, "P1 Toss (B+A+Y)");
        assert_eq!(mapping, "RightTrigger [pad] — no key");

        // Light = B is bound on both the key (Z) and the pad (South).
        let (label, mapping) = lines.iter().find(|(l, _)| l.contains("Light")).unwrap();
        assert_eq!(label, "P1 Light (B)");
        assert_eq!(mapping, "South [pad] — Z [key]");

        // Launcher = B+A: pad chord (Mode) but no key.
        let (_, mapping) = lines.iter().find(|(l, _)| l.contains("Launcher")).unwrap();
        assert_eq!(mapping, "Mode [pad] — no key");
    }
}


// ── action vocabulary (the controls contract, docs/game-profiles.md) ────────
//
// One resolver produces the per-port ACTION list every human-facing surface
// renders: the Controls panel, the calibration wizard, Help, and the Input
// monitor. Name resolution chain per action: game-profile name (attack
// classes/chords) → core-provided input descriptor → raw RETRO name.

/// Where an action's display name came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionSource {
    /// From the game profile (attack classes, coin/start, directions).
    Profile,
    /// From the core's RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS labels.
    Descriptor,
    /// Raw RETRO button name (no better information available).
    Retro,
}

/// One row of the game's control vocabulary: a named action and the RETRO
/// bits that perform it.
#[derive(Clone, Debug)]
pub struct ActionRow {
    pub name: String,
    pub bits: Vec<RetroButton>,
    pub source: ActionSource,
}

/// Build the action vocabulary for `port` (0/1). `descriptors` is
/// `DebugState::input_descriptors` (may be all-None: fbalpha2012 sends none).
///
/// Order: directions, Start, Coin, the profile's attack classes (in family
/// order, skipping "None"), then any remaining RETRO id the core described
/// that no prior row covers as a single button. Buttons neither profiled nor
/// described are OMITTED — the core not naming them is evidence the game
/// never reads them.
pub fn action_rows(
    port: usize,
    descriptors: &[[Option<String>; 12]; 2],
) -> Vec<ActionRow> {
    use RetroButton::*;
    let p = crate::profile::current();
    let desc = &descriptors[port.min(1)];
    let mut rows: Vec<ActionRow> = Vec::new();
    let mut single_covered = [false; 12];

    for (name, b) in [("Up", Up), ("Down", Down), ("Left", Left), ("Right", Right)] {
        rows.push(ActionRow { name: name.into(), bits: vec![b], source: ActionSource::Retro });
        single_covered[b.idx()] = true;
    }
    rows.push(ActionRow { name: "Start".into(), bits: vec![Start], source: ActionSource::Profile });
    rows.push(ActionRow { name: "Coin".into(), bits: vec![Select], source: ActionSource::Profile });
    single_covered[Start.idx()] = true;
    single_covered[Select.idx()] = true;

    // Profile attack classes, in family order.
    for class in &p.family.attack_classes {
        if class == "None" {
            continue;
        }
        if let Some(chord) = p.port.attack_chords.get(class) {
            let bits: Vec<RetroButton> = chord
                .iter()
                .filter_map(|n| RetroButton::from_retro_name(n))
                .collect();
            if bits.is_empty() {
                continue;
            }
            if bits.len() == 1 {
                single_covered[bits[0].idx()] = true;
            }
            rows.push(ActionRow { name: class.clone(), bits, source: ActionSource::Profile });
        }
    }

    // Core-described leftovers (e.g. a 6th button the profile doesn't model).
    const ALL: [RetroButton; 12] = [
        RetroButton::B, RetroButton::Y, RetroButton::Select, RetroButton::Start,
        RetroButton::Up, RetroButton::Down, RetroButton::Left, RetroButton::Right,
        RetroButton::A, RetroButton::X, RetroButton::L, RetroButton::R,
    ];
    for b in ALL {
        if !single_covered[b.idx()] {
            if let Some(label) = &desc[b.idx()] {
                rows.push(ActionRow {
                    name: label.clone(),
                    bits: vec![b],
                    source: ActionSource::Descriptor,
                });
            }
        }
    }
    rows
}
