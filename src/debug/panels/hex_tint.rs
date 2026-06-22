//! Helpers for tinting changed bytes in the hex viewer.
//! Consumed by the hex-dump changed-cell highlight (wired in a later wave).
#![allow(dead_code)]

use bevy_egui::egui;

/// Returns a highlight color for changed bytes in the hex viewer.
pub fn changed_color(changed: bool) -> egui::Color32 {
    if changed {
        egui::Color32::from_rgb(255, 180, 80)
    } else {
        egui::Color32::WHITE
    }
}

/// Compares two byte slices and returns a per-byte vector indicating which bytes changed.
///
/// If lengths differ, comparison goes up to the shorter length and missing bytes are
/// treated as unchanged (false).
pub fn diff_changed(prev: &[u8], cur: &[u8]) -> Vec<bool> {
    let min_len = prev.len().min(cur.len());
    let mut result = vec![false; min_len];

    for i in 0..min_len {
        result[i] = prev[i] != cur[i];
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_changed_equal_slices() {
        let prev = b"hello";
        let cur = b"hello";
        let changed = diff_changed(prev, cur);
        assert_eq!(changed, vec![false, false, false, false, false]);
    }

    #[test]
    fn test_diff_changed_one_byte_different() {
        let prev = b"hello";
        let cur = b"hallo";
        let changed = diff_changed(prev, cur);
        assert_eq!(changed, vec![false, true, false, false, false]);
    }
}
