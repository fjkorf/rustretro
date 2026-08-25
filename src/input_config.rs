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
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct PortMap {
    /// One key → one RETRO bit (several keys may share a bit).
    pub keyboard: BTreeMap<KeyCode, RetroButton>,
    /// One physical button → one-or-more RETRO bits (chords).
    pub gamepad: BTreeMap<GamepadButton, Vec<RetroButton>>,
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
        let f300: BTreeMap<G, Vec<RetroButton>> = [
            (G::South, vec![B]),
            (G::West, vec![A]),
            (G::North, vec![Y]),
            (G::East, vec![X]),
            (G::RightTrigger, vec![B, A, Y]), // weapon toss chord
            (G::Mode, vec![B, A]),            // launcher chord
            (G::LeftTrigger, vec![Select]),
            (G::RightTrigger2, vec![Start]),
            (G::LeftTrigger2, vec![Start]),
            (G::DPadUp, vec![Up]),
            (G::DPadDown, vec![Down]),
            (G::DPadLeft, vec![Left]),
            (G::DPadRight, vec![Right]),
        ]
        .into_iter()
        .collect();
        let p1_keys: BTreeMap<K, RetroButton> = [
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
        .collect();
        let p2_keys: BTreeMap<K, RetroButton> = [
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
        .collect();
        InputConfig {
            stick_deadzone: 0.5,
            pad_order: vec![0, 1],
            ports: vec![
                PortMap { keyboard: p1_keys, gamepad: f300.clone() },
                PortMap { keyboard: p2_keys, gamepad: f300 },
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
    let mut bits = [false; 12];
    for (btn, ids) in &port.gamepad {
        if pressed(*btn) {
            for id in ids {
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
    for (key, id) in &port.keyboard {
        if pressed(*key) {
            bits[id.idx()] = true;
        }
    }
    bits
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
}
